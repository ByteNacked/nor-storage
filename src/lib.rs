#![no_std]
#![allow(dead_code, unused_imports)]

#[macro_use]
extern crate static_assertions;
#[macro_use]
extern crate memoffset;

use core::marker::PhantomData;
use core::mem::size_of;
use core::slice::{from_raw_parts_mut, from_raw_parts};
use core::convert::TryInto;

pub mod prelude;

// ATTENTION: TODO: Deeply think about aligment of types
// TODO: add resered 0 tag to macro for record set version control
// TODO: fix convoluted tests and add corrupted mem test
// TODO: get rid of crc dependency

// Minimal addressing unit (and aligment)
pub type Word = u32;
// Header len in words
const HEADER_SZ: usize = size_of::<Header>();
// Word size in bytes
pub const WORD_SZ: usize = size_of::<Word>();

#[repr(C)]
#[derive(PartialEq, Eq, Debug)]
pub struct Header {
    tag: Word,
    /// Size of payload in bytes!
    sz:  Word,
    crc: Word,
}
const_assert!(HEADER_SZ % WORD_SZ == 0);
const_assert_eq!(
    core::mem::align_of::<Header>(), 
    core::mem::align_of::<Word>(), 
);

#[derive(Debug)]
pub enum Error<T> {
    OutOfMemory,
    CorruptedRecordOnGet,
    Crc,
    Driver(T),
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct RecordDesc {
    pub tag: Word,
    pub ptr: Option<(&'static Header, usize)>,
}

#[derive(Debug)]
pub struct InitStats {
    words_wasted: usize,
    unique_tags:  usize,
}

pub trait StorageMem {
    type Error;
    fn write(&mut self, offset_words: usize, word: Word) -> Result<(), Self::Error>;
    fn read(&self, offset_words : usize) -> Word;
    fn read_slice(&self, offset_start: usize, offset_end: usize) -> &'static [Word];
    fn len(&self) -> usize;
}

pub trait StorageHasher32 {
    fn reset(&mut self);
    fn write32(&mut self, words: &[u32]);
    fn finish(&self) -> u32;
}

pub struct Storage<S, H> {
    storage: S,
    cur_word: usize,
    _p: PhantomData<H>,

}

impl<S, H> Storage<S, H> 
where 
    S: StorageMem,
    H: StorageHasher32,
{

    pub fn new(storage: S) -> Self {
        Self {
            storage,
            cur_word: 0,
            _p: PhantomData,
        }
    }
    
    /// Scan through storage memory and populate record descriptor table
    pub fn init(&mut self, list: &mut [RecordDesc], hasher: &mut H) -> InitStats {

        let mut stats = InitStats { words_wasted : 0, unique_tags : 0 };
        
        let mut idx = 0;
        let mut size;
        let mut last_valid_end = 0;
        let capacity = self.storage.len();
        
        // Scanning through whole storage to find all valid records
        while idx < capacity - HEADER_SZ / WORD_SZ {
            let res = self.validate_record(idx, hasher);
            match res {
                Some(header) => {
                    assert_eq!(list[header.tag as usize].tag, header.tag, "Index in table should match tag!");
                    list[header.tag as usize].ptr = Some((header, idx));
                    let payload_sz_in_words = convert_sz_in_words(header.sz as usize);
                    idx += HEADER_SZ / WORD_SZ + payload_sz_in_words;
                    last_valid_end = idx;
                }
                None => {
                    idx += 1;
                }
            }
        }
        size = last_valid_end;
        
        // Scannig from last record end position, to determine that
        // rest flash memory wasn't already written (NOT 0xFF'ed)
        for idx in last_valid_end .. capacity {
            if !Self::is_ffed(self.storage.read(idx)) {
                size = idx + 1;
                stats.words_wasted += 1;
            }
        }

        self.cur_word = size;

        // Stats
        for e in list {
            if let Some(_) = &e.ptr {
                stats.unique_tags += 1;
            }
        }
        
        stats
    }

    fn validate_record(&self, idx: usize, hasher: &mut H) -> Option<&'static Header> {
        let _tag = self.storage.read(idx);
        let len_in_bytes = self.storage.read(idx + offset_of!(Header, sz) / WORD_SZ);
        let len_in_words = convert_sz_in_words(len_in_bytes as usize);
        let crc = self.storage.read(idx + offset_of!(Header, crc) / WORD_SZ);

        let payload_start_idx = idx + 3;
        let payload_end_idx = payload_start_idx.saturating_add(len_in_words);
        // Check payload slice is not out of bounds
        if payload_end_idx > self.storage.len() {
            return None;
        }
        
        // Calculate checksum
        hasher.reset();
        let header_part = self.storage.read_slice(idx, idx + offset_of!(Header, crc) / WORD_SZ);
        hasher.write32(header_part);
        let payload_slice = self.storage.read_slice(payload_start_idx, payload_end_idx);
        hasher.write32(payload_slice);
        
        // Compare checksums
        let calc_crc = hasher.finish();
        if crc != calc_crc {
            return None;
        }
        
        let header : &Header = unsafe { &*(self.storage.read_slice(idx, idx).as_ptr() as *const _) };
        Some(header)
    }
    

    // TODO: what if payload slice not Word size aligned?
    /// Update recordy entry
    pub fn update(&mut self, record: &mut RecordDesc, payload: &[u8], hasher: &mut H)
        -> Result<(),Error<S::Error>> 
    {
        let payload_len = payload.len();
        let record_len = HEADER_SZ + payload_len;
        if self.free_space() < record_len {
            return Err(Error::OutOfMemory);
        }

        let header_idx = self.cur_word;
        // Fill header
        self.storage.write(header_idx + offset_of!(Header, tag) / WORD_SZ, record.tag)
            .map_err(|e|Error::Driver(e))?;
        self.storage.write(header_idx + offset_of!(Header, sz) / WORD_SZ,  payload_len as Word)
            .map_err(|e|Error::Driver(e))?;

        let payload_idx = header_idx + HEADER_SZ / WORD_SZ;
        // Copy payload word by word
        for idx in 0 .. payload_len / WORD_SZ {
            let word = &payload[idx * WORD_SZ ..][ .. WORD_SZ];
            let word = Word::from_le_bytes(word.try_into().expect("Slice can not be converted"));
            self.storage.write(payload_idx + idx, word)
                .map_err(|e|Error::Driver(e))?;
        }
        // Residual bytes
        if payload_len % WORD_SZ != 0 {
            // FIXME: its horrible
            const_assert_eq!(WORD_SZ, size_of::<u32>());
            const FILL: u8 = 0xA5;
            let word = match payload_len % WORD_SZ {
                1 => [payload[payload_len - 1], FILL, FILL, FILL],
                2 => [payload[payload_len - 2], payload[payload_len - 1], FILL, FILL],
                3 => [payload[payload_len - 3], payload[payload_len - 2], payload[payload_len - 1], FILL],
                _ => unreachable!(),
            };
            let word = Word::from_le_bytes(word.try_into().expect("Slice can not be converted"));
            self.storage.write(payload_idx + payload_len / WORD_SZ, word)
                .map_err(|e|Error::Driver(e))?;
        }
        
        // Calculate and set checksum
        hasher.reset();
        hasher.write32(self.storage.read_slice(header_idx, header_idx + offset_of!(Header, crc) / WORD_SZ));
        hasher.write32(self.storage.read_slice(payload_idx, payload_idx + payload.len() / WORD_SZ));
        // Residual
        if payload_len % WORD_SZ != 0 {
            hasher.write32(&[self.storage.read(payload_idx + payload.len() / WORD_SZ)]);
        }
        let checksum = hasher.finish();
        self.storage.write(header_idx + offset_of!(Header, crc) / WORD_SZ, checksum)
            .map_err(|e|Error::Driver(e))?;

        // Update record descriptor
        let updated_header : &Header = unsafe { &*(self.storage.read_slice(header_idx, header_idx).as_ptr() as *const Header) };
        record.ptr = Some((updated_header, header_idx));

        // Update cur_word len
        self.cur_word += convert_sz_in_words(record_len);

        Ok(())
    }
    
    // TODO: what if result is not Word size aligned?
    /// Get record payload
    pub fn get(&self, record: &RecordDesc, hasher: Option<&mut H>)
        -> Result<Option<&'static [u8]>,Error<S::Error>> 
    {
        match record.ptr {
            Some((header, idx)) => {
                // Basic sanity check
                if header.tag != record.tag { return Err(Error::CorruptedRecordOnGet); }

                //Crc check 
                if let Some(hasher) = hasher {
                    let _ = self.validate_record(idx, hasher).ok_or(Error::Crc)?;
                }

                unsafe {
                    let header_ptr = header as *const _ as *const u8;
                    let payload_ptr = header_ptr.offset(HEADER_SZ as isize);
                    Ok(Some(from_raw_parts(payload_ptr, header.sz as usize)))
                }
            },
            None => Ok(None),
        }
    }

    /// Total amount of occupied storage space in bytes
    pub fn len(&self) -> usize {
        self.cur_word * WORD_SZ
    }
    /// Total storage space in bytes
    pub fn capacity(&self) -> usize {
        self.storage.len() * WORD_SZ
    }

    fn free_space(&self) -> usize {
        self.capacity() - self.cur_word * WORD_SZ
    }

    fn is_ffed(word : Word) -> bool {
        if word == !0 {
            return true;
        }
        return false;
    }
}

fn convert_sz_in_words(sz_in_bytes: usize) -> usize {
    if sz_in_bytes % WORD_SZ == 0 {
        sz_in_bytes / WORD_SZ
    } else {
        sz_in_bytes / WORD_SZ + 1
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

        fn write32(&mut self, words: &[u32]) {
            let bytes = unsafe { 
                from_raw_parts(words.as_ptr() as *const u8, words.len() * WORD_SZ) 
            };
            <Digest as Hasher32>::write(self, bytes);
        }

        fn finish(&self) -> u32 {
            <Digest as Hasher32>::sum32(self)
        }
    }

    pub struct TestMem ( pub [Word;0x100] );

    impl StorageMem for TestMem {
        type Error = ();

        fn write(&mut self, offset_words : usize, word : Word) -> Result<(), Self::Error> {
            if self.0[offset_words] == !0 {
                Ok(self.0[offset_words] = word)
            } else {
                Err(())
            }
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
    use core::fmt::{self, Display};

    impl Display for Storage<TestMem, Digest> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for i in (0 .. 0x40).step_by(4) {
                writeln!(f, "{}: {:x?}", i, &self.storage.0[i..][..4])?;
            }
            Ok(())
        }
    }
    
    fn crc32_new() -> Digest {
        Digest::new_custom(IEEE, !0u32, 0u32, CalcType::Normal)
    }

    fn new_storage() -> Storage<TestMem, Digest> {
        Storage::new(TestMem([!0;0x100]))
    }

    #[test]
    fn empty_test() {
        let storage_mem = [!0u32;0x100];
        let capacity = storage_mem.len() * size_of::<Word>();
        let storage = Storage::<_, Digest>::new(TestMem(storage_mem));

        assert_eq!(storage.len(), 0);
        assert_eq!(storage.capacity(), capacity);
    }

    #[test]
    fn reinit_test() {
        let storage_mem = [!0u32;0x100];
        let mut storage = Storage::<_, Digest>::new(TestMem(storage_mem));
        let mut crc32 = crc32_new();

        let mut rec_desc = [
            RecordDesc {
                tag : 0,
                ptr : None,
            },
            RecordDesc {
                tag : 1,
                ptr : None,
            },
        ];

        let rec_payload = b"test";
        storage.init(&mut rec_desc, &mut crc32);
        storage.update(&mut rec_desc[1], &rec_payload[..], &mut crc32).unwrap();
        assert!(&rec_desc[1].ptr.is_some());

        let mut rec_desc = [
            RecordDesc {
                tag : 0,
                ptr : None,
            },
            RecordDesc {
                tag : 1,
                ptr : None,
            },
        ];
        let rec_payload = b"foo";
        storage.init(&mut rec_desc, &mut crc32);
        storage.update(&mut rec_desc[1], &rec_payload[..], &mut crc32).unwrap();
        assert!(&rec_desc[1].ptr.is_some());
        
        assert_eq!(storage.get(&mut rec_desc[1], Some(&mut crc32)).unwrap().unwrap(), b"foo");
    }
    
    #[test]
    fn new_record_test() {
        let mut storage = new_storage();
        let mut rec_desc = RecordDesc {
            tag : 1,
            ptr : None,
        };

        let rec_payload = [42u8;1];
        let mut crc32 = crc32_new();
        
        storage.update(&mut rec_desc, &rec_payload, &mut crc32).unwrap();
        assert_eq!(storage.len(), (HEADER_SZ + convert_sz_in_words(rec_payload.len()) * WORD_SZ));
        assert!(&rec_desc.ptr.is_some());
        
        let out_rec_payload = storage.get(&rec_desc, Some(&mut crc32)).unwrap().unwrap();
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
        let mut crc32 = crc32_new();

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

        let e0 = [!42u8; 10];
        storage.update(&mut desc_list[0], &e0, &mut crc32).unwrap();

        let e1 = [0x77u8; 3];
        storage.update(&mut desc_list[1], &e1, &mut crc32).unwrap();

        let e0 = [0x66u8; 3];
        storage.update(&mut desc_list[0], &e0, &mut crc32).unwrap();

        let e2 = [0x55u8; 4];
        storage.update(&mut desc_list[2], &e2, &mut crc32).unwrap();

        let e0 = [0xB5u8, 0xA5, 0x7E];
        storage.update(&mut desc_list[0], &e0, &mut crc32).unwrap();

        let e1 = [66u8; 5];
        storage.update(&mut desc_list[1], &e1, &mut crc32).unwrap();
        
        let mut ndesc_list = [
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
        storage.init(&mut ndesc_list, &mut crc32);
        assert_eq!(storage.get(&ndesc_list[0], Some(&mut crc32)).unwrap().unwrap(), &e0);
        assert_eq!(storage.get(&ndesc_list[1], Some(&mut crc32)).unwrap().unwrap(), &e1);
        assert_eq!(storage.get(&ndesc_list[2], Some(&mut crc32)).unwrap().unwrap(), &e2);

        assert_eq!(&desc_list, &ndesc_list)
        
        //println!("Desc list : {:#?}", &desc_list);
    }

    #[test]
    fn oom_test() {
        let mut storage = new_storage();
        let mut crc32 = crc32_new();

        let mut desc_list = [
            RecordDesc {
                tag : 0,
                ptr : None,
            },
        ];

        let e0 = [!42u8; 10];
        while let Err(e) = storage.update(&mut desc_list[0], &e0, &mut crc32) {
            if let Error::OutOfMemory = e {

            } else { panic!() }
        }
    }


    #[test]
    #[ignore]
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
