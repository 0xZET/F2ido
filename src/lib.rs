extern crate cryptoki;
extern crate packed_struct;
extern crate serde_cbor;

#[macro_use]
pub mod log;

mod binio;
pub mod crypto;
mod ctaphid;
mod error;
//mod eventloop;
mod hid;
//mod panic;
mod ctap2;
mod hex;
pub mod prompt;
mod u2f;
mod usb;
pub mod usbip;

pub mod bindings {
    #![allow(non_upper_case_globals)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(dead_code)]

    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}
