#![no_std]
#![allow(dead_code, unused_imports)]

use core::mem::size_of;
use core::slice::{from_raw_parts_mut, from_raw_parts};

pub mod prelude;

// TODO: implement errors and error tests
// TODO: validity check on fn get

// Minimal addressing unit (and aligment)
pub type Word = u32;
// Header len in words
const HEADER_LEN : usize = size_of::<Header>() / size_of::<Word>();
// Word size in bytes
pub const WORD_SIZE : usize = size_of::<Word>();

#[repr(C)]
#[derive(PartialEq, Eq, Debug)]
pub struct Header {
    tag  : Word,
    /// Size of payload in words
    sz   : Word,
    crc  : u32,
}

#[derive(Debug)]
pub enum Error {
    OutOfFreeSpace,
    CorruptedRecordOnGet,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct RecordDesc {
    pub tag : Word,
    pub ptr : Option<&'static Header>,
}

#[derive(Debug)]
pub struct InitStats {
    words_wasted : usize,
    unique_tags  : usize,
}

pub trait StorageMem {
    type Error;
    fn write(&mut self, offset_words : usize, word : Word) -> Result<(), Self::Error>;
    fn read(&self, offset_words : usize) -> Word;
    fn read_slice(&self, offset_start : usize, offset_end : usize) -> &'static [Word];
    fn len(&self) -> usize;
}

pub trait StorageHasher32 {
    fn reset(&mut self);
    fn write(&mut self, words: &[u32]);
    fn sum(&self) -> u32;
}

pub struct Storage<S> {
    storage : S,
    current : usize,
}

impl<S : StorageMem> Storage<S> {

    pub fn new(storage : S) -> Self {
        Self {
            storage,
            current : 0,
        }
    }
    
    /// Scan through storage memory and populate record descriptor table
    pub fn init(&mut self, list : &mut [RecordDesc], hasher : &mut impl StorageHasher32) -> InitStats {

        let mut stats = InitStats { words_wasted : 0, unique_tags : 0 };
        
        let mut idx = 0;
        let mut size = 0;
        let mut last_valid_end = 0;
        let capacity = self.storage.len();
        
        // Scanning through whole storage to find all valid records
        while idx < capacity - HEADER_LEN {
            let res = self.validate_record(idx, hasher);
            match res {
                Some(header) => {
                    assert_eq!(list[header.tag as usize].tag, header.tag, "Index in table should match tag!");
                    list[header.tag as usize].ptr = Some(header);
                    idx += HEADER_LEN + header.sz as usize;
                    last_valid_end = idx;
                }
                None => {
                    idx += 1;
                }
            }
        }
        
        // Scannig from last record end position, to determine that
        // rest flash memory wasn't already written (NOT 0xFF'ed)
        for idx in last_valid_end .. capacity {
            if !Self::is_ffed(self.storage.read(idx)) {
                size = idx + 1;
                stats.words_wasted += 1;
            }
        }

        self.current = size;

        // Stats
        for e in list {
            if let Some(_) = &e.ptr {
                stats.unique_tags += 1;
            }
        }
        
        stats
    }

    fn validate_record(&self, idx : usize, hasher : &mut impl StorageHasher32) -> Option<&'static Header> {
        let _tag = self.storage.read(idx);
        let len = self.storage.read(idx + 1);
        let crc = self.storage.read(idx + 2);

        let payload_start_idx = idx + 3;
        let payload_end_idx = payload_start_idx.saturating_add(len as usize);
        // Check payload slice is not out of bounds
        if payload_end_idx > self.storage.len() {
            return None;
        }
        
        // Calculate checksum
        hasher.reset();
        let header_part = self.storage.read_slice(idx, idx + 2);
        hasher.write(header_part);
        let payload_slice = self.storage.read_slice(payload_start_idx, payload_end_idx);
        hasher.write(payload_slice);
        
        // Compare checksums
        let calc_crc = hasher.sum();
        if crc != calc_crc {
            return None;
        }
        
        let header : &Header = unsafe { &*(self.storage.read_slice(idx, idx).as_ptr() as *const _) };
        Some(header)
    }
    
    /// Update recordy entry
    pub fn update(&mut self, record : &mut RecordDesc, payload : &[Word], hasher : &mut impl StorageHasher32) -> Result<(),Error> {
        let record_len = HEADER_LEN + payload.len();
        if self.free_space_in_words() < record_len {
            return Err(Error::OutOfFreeSpace);
        }

        let header_idx = self.current;
        // Fill header
        assert!(self.storage.write(header_idx + 0, record.tag).is_ok());
        assert!(self.storage.write(header_idx + 1, payload.len() as Word).is_ok());

        let payload_idx = header_idx + HEADER_LEN;
        // Copy payload
        for idx in 0 .. payload.len() {
            assert!(self.storage.write(payload_idx + idx, payload[idx]).is_ok());
        }
        
        // Calculate and set checksum
        hasher.reset();
        hasher.write(self.storage.read_slice(header_idx, header_idx + 2));
        hasher.write(self.storage.read_slice(payload_idx, payload_idx + payload.len()));
        let checksum = hasher.sum();
        assert!(self.storage.write(header_idx + 2, checksum).is_ok());

        // Update record descriptor
        let updated_header : &Header = unsafe { &*(self.storage.read_slice(header_idx, header_idx).as_ptr() as *const Header) };
        record.ptr = Some(updated_header);

        // Update current len
        self.current += record_len;

        Ok(())
    }
    
    /// Get record payload
    pub fn get(&self, record : &RecordDesc) -> Result<Option<&'static [u32]>,Error> {
        match record.ptr {
            Some(header) => {
                // Basic sanity check
                if header.tag == record.tag {
                    unsafe {
                        let header_ptr = header as *const _ as *const u32;
                        let payload_ptr = header_ptr.offset(HEADER_LEN as isize);
                        Ok(Some(from_raw_parts(payload_ptr, header.sz as usize)))
                    }
                } else {
                    Err(Error::CorruptedRecordOnGet)
                }
            },
            None => Ok(None),
        }
    }

    /// Total amount of occupied storage space in bytes
    pub fn len(&self) -> usize {
        self.current * WORD_SIZE
    }
    /// Total storage space in bytes
    pub fn capacity(&self) -> usize {
        self.storage.len() * WORD_SIZE
    }

    fn free_space_in_words(&self) -> usize {
        self.storage.len() - self.current
    }

    fn is_ffed(word : Word) -> bool {
        if word == !0 {
            return true;
        }
        return false;
    }

    fn header_from_slice(&self, _slice : &'static [u32]) -> &'static Header {
        todo!()
    }

    fn payload_from_header_slice(&self, _header : &'static Header) -> Result<&'static [u32],()> {
        todo!()
    }
}

#[cfg(any(test, feature="test-def"))]
pub use test_def::TestMem;

#[cfg(any(test, feature="test-def"))]
mod test_def {
    use super::*;

    pub use crc::crc32::{Digest, Hasher32};

    impl StorageHasher32 for Digest {
        fn reset(&mut self) {
            <Digest as Hasher32>::reset(self);
        }

        fn write(&mut self, words: &[u32]) {
            let bytes = unsafe { 
                from_raw_parts(words.as_ptr() as *const u8, words.len() * WORD_SIZE) 
            };
            <Digest as Hasher32>::write(self, bytes);
        }

        fn sum(&self) -> u32 {
            <Digest as Hasher32>::sum32(self)
        }
    }

    pub struct TestMem ( pub [Word;0x100] );

    impl StorageMem for TestMem {
        type Error = ();

        fn write(&mut self, offset_words : usize, word : Word) -> Result<(), Self::Error> {
            Ok(self.0[offset_words] = word)
        }

        fn read(&self, offset_words : usize) -> Word {
            self.0[offset_words]
        }

        fn read_slice(&self, offset_start : usize, offset_end : usize) -> &'static [Word] {
            unsafe { core::mem::transmute(&self.0[offset_start .. offset_end]) }
        }

        fn len(&self) -> usize {
            self.0.len()
        }
    }
}


#[allow(dead_code, unused_imports)]
#[cfg(test)]
mod tests {
    use super::*;
    use crc::crc32::{Digest, IEEE_TABLE, IEEE, Hasher32};
    use crc::CalcType;
    
    fn crc32_ethernet() -> impl StorageHasher32 {
        Digest::new_custom(IEEE, !0u32, 0u32, CalcType::Normal)
    }

    fn new_storage() -> Storage<TestMem> {
        Storage::new(TestMem([!0;0x100]))
    }

    #[test]
    fn empty_test() {
        let storage_mem = [!0u32;0x100];
        let capacity = storage_mem.len() * size_of::<Word>();
        let storage = Storage::new(TestMem(storage_mem));

        assert_eq!(storage.len(), 0);
        assert_eq!(storage.capacity(), capacity);
    }
    
    #[test]
    fn new_record_test() {
        let mut storage = new_storage();
        let mut rec_desc = RecordDesc {
            tag : 1,
            ptr : None,
        };

        let rec_payload = [42u32;1];
        let mut crc32 = crc32_ethernet();
        
        storage.update(&mut rec_desc, &rec_payload, &mut crc32).unwrap();
        assert_eq!(storage.len(), (HEADER_LEN + rec_payload.len()) * WORD_SIZE );
        assert!(&rec_desc.ptr.is_some());
        
        let out_rec_payload = storage.get(&rec_desc).unwrap().unwrap();
        //println!("Desc list : {:#?}", &rec_desc);
        assert_eq!(&rec_payload, out_rec_payload);


        let mut desc_list = [
            RecordDesc {
                tag : 0,
                ptr : None,
            },
            RecordDesc {
                tag : 1,
                ptr : None,
            },
        ];
        let _stats = storage.init(&mut desc_list, &mut crc32);
        assert_eq!(&desc_list[1], &rec_desc);
        //println!("Desc list : {:#?}", &desc_list);
    }

    #[test]
    fn series_of_records_test() {
        let mut storage = new_storage();
        let mut crc32 = crc32_ethernet();

        let mut desc_list = [
            RecordDesc {
                tag : 0,
                ptr : None,
            },
            RecordDesc {
                tag : 1,
                ptr : None,
            },
            RecordDesc {
                tag : 2,
                ptr : None,
            },
        ];

        let e0 = [!42u32; 10];
        storage.update(&mut desc_list[0], &e0, &mut crc32).unwrap();

        let e1 = [0x7777_7777; 3];
        storage.update(&mut desc_list[1], &e1, &mut crc32).unwrap();

        let e0 = [0x6666_6666; 3];
        storage.update(&mut desc_list[0], &e0, &mut crc32).unwrap();

        let e2 = [0x5555_5555; 3];
        storage.update(&mut desc_list[2], &e2, &mut crc32).unwrap();

        let e0 = [0xA5B5A5A5u32; 2];
        storage.update(&mut desc_list[0], &e0, &mut crc32).unwrap();

        let e1 = [66u32; 5];
        storage.update(&mut desc_list[1], &e1, &mut crc32).unwrap();
        
        storage.init(&mut desc_list, &mut crc32);
        assert_eq!(storage.get(&desc_list[0]).unwrap().unwrap(), &e0);
        assert_eq!(storage.get(&desc_list[1]).unwrap().unwrap(), &e1);
        assert_eq!(storage.get(&desc_list[2]).unwrap().unwrap(), &e2);
        
        //println!("Desc list : {:#?}", &desc_list);
    }

    #[test]
    fn crc32_test() {

        // CRC-32/MPEG-2 
        let mut crc = Digest::new_custom(IEEE, !0u32, 0u32, CalcType::Normal);

        //Hasher32::reset(&mut crc);
        //let b = [0xA5u8];
        //Hasher32::write(&mut crc, &b);
        //let res : u32 = crc.sum32();
        //println!("\n{:x}\n", &res);
        //assert_eq!(res, 0xA8E282D1);

        //Hasher32::reset(&mut crc);
        //let b = [0xA5u8, 0];
        //Hasher32::write(&mut crc, &b);
        //let res : u32 = crc.sum32();
        //println!("\n{:x}\n", &res);
        //assert_eq!(res, 0xA8E282D1);
        
        Hasher32::reset(&mut crc);
        let b = [0xA5,0xA5,0xA5,0xA5];
        Hasher32::write(&mut crc, &b);
        let res : u32 = crc.sum32();
        assert_eq!(res, 0x29928E70);
    }

}
