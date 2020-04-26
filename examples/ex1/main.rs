// Write code here.
//
// To see what the code looks like after macro expansion:
//     $ cargo expand
//
// To run the code:
//     $ cargo run


use crc::crc32::{Digest,IEEE};
use crc::CalcType;

use nor_storage::prelude::*;

#[derive(Debug)]
pub enum Mode {
    InAir,
    Lifting,
    Landing,
    OnGround,
}

generate_storage_ty! {
    struct PerMap {
        name : u32,
        calib : u32,
        calib2 : u16,
        sign : u8,
        num : u8,
        cara : u8,
        flag : bool,
        //barray : [bool;5],
        mode : Mode,
        my_str: &'static str,
        my_bytes: &'static [u8],
    }
}

fn crc32_ethernet() -> impl StorageHasher32 {
    Digest::new_custom(IEEE, !0u32, 0u32, CalcType::Normal)
}

fn main() {
    
    let mem = nor_storage::TestMem([!0;0x100]);

    let mut storage = PerMap::new(mem);
    let mut crc = crc32_ethernet();
    let _ = storage.init(&mut crc);
    
    storage.set_name(7u32, &mut crc).unwrap();
    storage.set_name(6u32, &mut crc).unwrap();
    storage.set_name(3u32, &mut crc).unwrap();
    storage.set_name(1u32, &mut crc).unwrap();
    storage.set_my_str("Hello", &mut crc).unwrap();
    storage.set_calib(777u32, &mut crc).unwrap();
    storage.set_cara(42u8, &mut crc).unwrap();
    storage.set_cara(42u8, &mut crc).unwrap();
    storage.set_cara(42u8, &mut crc).unwrap();
    storage.set_flag(true, &mut crc).unwrap();
    storage.set_flag(false, &mut crc).unwrap();
    storage.set_my_str("Crabby crab", &mut crc).unwrap();
    //storage.set_barray([false, true, false, true, true], &mut crc).unwrap();
    //storage.set_barray([false; 5], &mut crc).unwrap();
    storage.set_mode(Mode::Lifting, &mut crc).unwrap();
    storage.set_mode(Mode::InAir, &mut crc).unwrap();
    
    storage.set_my_bytes(&[0u8,1,2], &mut crc).unwrap();
    storage.set_my_bytes(&[2u8,1,0], &mut crc).unwrap();

    let stats = storage.init(&mut crc);
    println!("Stats: {:#?}", stats);
    let mut fmt_str = String::new();
    let _ = storage.format(&mut fmt_str, &mut crc);
    println!("{}", &fmt_str);
}




