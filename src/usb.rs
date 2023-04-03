use crate::bindings::*;
use crate::binio::{write_struct, write_struct_limited};
use crate::crypto::Token;
use crate::ctaphid;
use crate::error::{IOR, R};
use crate::eventloop;
use crate::hid;
use crate::prompt;
use packed_struct::prelude::*;
use packed_struct::PackedStruct;
use packed_struct::PrimitiveEnum;
use std::convert::TryFrom;
use std::io::Write;
use std::mem::size_of;

const LANG_ID_EN_US: u16 = 0x0409;

// const URB_SHORT_NOT_OK        = 0x00000001;
// const URB_ISO_ASAP            = 0x00000002;
// const URB_NO_TRANSFER_DMA_MAP = 0x00000004;
// const URB_ZERO_PACKET         = 0x00000040;
// const URB_NO_INTERRUPT        = 0x00000080;
// const URB_FREE_BUFFER         = 0x00000100;
pub const URB_DIR_MASK: u32 = 0x00000200;

#[derive(PackedStruct, Clone, Copy, Debug)]
#[packed_struct(endian = "lsb")]
pub struct SetupPacket {
    #[packed_field(size_bytes = "1")]
    bm_request_type: BmRequestType,
    b_request: u8,
    w_value: u16,
    w_index: u16,
    w_length: u16,
}

impl SetupPacket {
    fn request_type(self) -> (DataTransferDirection, RT, RR) {
        (
            self.bm_request_type.direction,
            self.bm_request_type.type_,
            self.bm_request_type.recipient,
        )
    }
    fn args(self) -> (u16, u16, u16) {
        (
            u16::from_le(self.w_value),
            u16::from_le(self.w_index),
            u16::from_le(self.w_length),
        )
    }

    fn std(self) -> StandardRequest {
        StandardRequest::from_primitive(self.b_request).unwrap()
    }

    fn hid_request(self) -> (HIDRequest, (u16, u16, u16)) {
        (
            HIDRequest::from_primitive(self.b_request).unwrap(),
            self.args(),
        )
    }
}

#[derive(PackedStruct, Clone, Copy, Debug)]
#[packed_struct(endian = "lsb", size_bytes = "1", bit_numbering = "lsb0")]
pub struct BmRequestType {
    #[packed_field(bits = "7", ty = "enum")]
    direction: DataTransferDirection,
    #[packed_field(bits = "5..=6", ty = "enum")]
    type_: RequestType,
    #[packed_field(bits = "0..=4", ty = "enum")]
    recipient: RequestRecipient,
}

#[derive(PrimitiveEnum, Clone, Copy, Debug)]
pub enum DataTransferDirection {
    HostToDevice = 0,
    DeviceToHost = 1,
}

use DataTransferDirection::DeviceToHost as D2H;
use DataTransferDirection::HostToDevice as H2D;

#[derive(PrimitiveEnum, Clone, Copy, Debug)]
pub enum RequestType {
    Standard = 0,
    Class = 1,
    Vendor = 2,
    Reserved = 3,
}

use RequestType as RT;

#[derive(PrimitiveEnum, Clone, Copy, Debug)]
pub enum RequestRecipient {
    Device = 0,
    Interface = 1,
    Endpoint = 2,
    Other = 3,
}

use RequestRecipient as RR;

#[derive(PrimitiveEnum_u8, Clone, Copy, Debug, PartialEq)]
pub enum StandardRequest {
    GetStatus = 0,
    ClearFeature = 1,
    SetFeature = 3,
    SetAddress = 5,
    GetDescriptor = 6,
    SetDescriptor = 7,
    GetConfiguration = 8,
    SetConfiguration = 9,
    GetInterface = 10,
    SetInterface = 11,
    SynchFrame = 12,
}

use StandardRequest as SR;

#[derive(PrimitiveEnum_u8, Clone, Copy, Debug, PartialEq)]
pub enum HIDRequest {
    GetReport = 1,
    GetIdle = 2,
    GetProtocol = 3,
    SetReport = 9,
    SetIdle = 0xa,
    SetProtocol = 0xb,
}

#[derive(PrimitiveEnum, Clone, Copy, Debug)]
enum DescriptorType {
    Device = 1,
    Configuration = 2,
    String = 3,
    Interface = 4,
    Endpoint = 5,
    DeviceQualifier = 6,
    OtherSpeedConfiguration = 7,
    InterfacePower = 8,
}

use DescriptorType as DT;

#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
#[allow(non_snake_case)]
struct usb_hid_descriptor {
    bLength: u8,
    bDescriptorType: u8,
    bcdHID: u16,
    bCountryCode: u8,
    bNumDescriptors: u8,
    bReportDescriptorType: u8,
    wReportDescriptorLength: u16,
    // ... optional other descriptors type/length pairs
}

pub struct Device<'a> {
    pub device_descriptor: usb_device_descriptor,
    pub config_descriptor: usb_config_descriptor,
    pub interface_descriptor: usb_interface_descriptor,
    hid_descriptor: usb_hid_descriptor,
    hid_report_descriptor: Vec<u8>,
    endpoint_descriptors: Vec<usb_endpoint_descriptor>,
    strings: Vec<&'static str>,
    parser: ctaphid::Parser<'a>,
}

// USB Request Block
pub struct URB<T> {
    pub endpoint: u8,
    pub setup: SetupPacket,
    pub transfer_buffer: Vec<u8>,
    pub complete: Option<Box<dyn FnOnce(Box<URB<T>>)>>,
    pub context: Box<T>,
    pub status: Option<R<bool>>, //bool is temporary
}

impl<'a> Device<'a> {
    pub fn new(token: &'a Token, prompt: &'a dyn prompt::Prompt) -> Self {
        let hid_report_descriptor: Vec<u8> = {
            use hid::*;
            [
                usage_page(FIDO),
                usage(CTAPHID),
                collection(APPLICATION),
                usage(FIDO_USAGE_DATA_IN),
                logical_minimum(0),
                logical_maximum(0xff),
                report_size(8),
                report_count(64),
                input(DATA | VARIABLE | ABSOLUTE),
                usage(FIDO_USAGE_DATA_OUT),
                logical_minimum(0),
                logical_maximum(0xff),
                report_size(8),
                report_count(64),
                output(DATA | VARIABLE | ABSOLUTE),
                end_collection(),
            ]
            .into_iter()
            .flatten()
            .collect()
        };

        Self {
            device_descriptor: usb_device_descriptor {
                bLength: size_of::<usb_device_descriptor>() as u8,
                bDescriptorType: DT::Device.to_primitive(),
                bcdUSB: 0x0110u16.to_le(),
                bDeviceClass: USB_CLASS_PER_INTERFACE as u8,
                bDeviceSubClass: 0,
                bDeviceProtocol: 0,
                bMaxPacketSize0: 64,
                idVendor: 0,
                idProduct: 0,
                bcdDevice: 0x001u16.to_le(),
                iManufacturer: 1,
                iProduct: 2,
                iSerialNumber: 3,
                bNumConfigurations: 1,
            },
            config_descriptor: usb_config_descriptor {
                bLength: size_of::<usb_config_descriptor>() as u8,
                bDescriptorType: DT::Configuration.to_primitive(),
                wTotalLength: u16::try_from(
                    size_of::<usb_config_descriptor>()
                        + size_of::<usb_interface_descriptor>()
                        + size_of::<usb_hid_descriptor>()
                        //+ hid_report_descriptor.len()
                        + 2 * USB_DT_ENDPOINT_SIZE as usize,
                )
                .unwrap()
                .to_le(),
                bNumInterfaces: 1,
                bConfigurationValue: 0,
                iConfiguration: 4,
                bmAttributes: (USB_CONFIG_ATT_ONE
                    | USB_CONFIG_ATT_SELFPOWER)
                    as u8,
                bMaxPower: 0,
            },
            interface_descriptor: usb_interface_descriptor {
                bLength: size_of::<usb_interface_descriptor>() as u8,
                bDescriptorType: DT::Interface.to_primitive(),
                bInterfaceNumber: 0,
                bAlternateSetting: 0,
                bNumEndpoints: 2,
                bInterfaceClass: USB_CLASS_HID as u8,
                bInterfaceSubClass: 0,
                bInterfaceProtocol: 0,
                iInterface: 5,
            },
            hid_descriptor: usb_hid_descriptor {
                bLength: size_of::<usb_hid_descriptor>() as u8,
                bDescriptorType: HID_DT_HID as u8,
                bcdHID: 0x101u16.to_le(),
                bCountryCode: 0,
                bNumDescriptors: 1,
                bReportDescriptorType: HID_DT_REPORT as u8,
                wReportDescriptorLength: (hid_report_descriptor.len()
                    as u16)
                    .to_le(),
            },
            hid_report_descriptor: hid_report_descriptor,
            endpoint_descriptors: vec![
                usb_endpoint_descriptor {
                    bLength: USB_DT_ENDPOINT_SIZE as u8,
                    bDescriptorType: DT::Endpoint.to_primitive(),
                    bEndpointAddress: ((1 & USB_ENDPOINT_NUMBER_MASK)
                        | (USB_DIR_IN & USB_ENDPOINT_DIR_MASK))
                        as u8,
                    bmAttributes: USB_ENDPOINT_XFER_INT as u8,
                    wMaxPacketSize: (((64 & USB_ENDPOINT_MAXP_MASK)
                        as u16)
                        .to_le()),
                    bInterval: 255,
                    bRefresh: 0,
                    bSynchAddress: 0,
                },
                usb_endpoint_descriptor {
                    bLength: USB_DT_ENDPOINT_SIZE as u8,
                    bDescriptorType: DT::Endpoint.to_primitive(),
                    bEndpointAddress: ((2 & USB_ENDPOINT_NUMBER_MASK)
                        | (USB_DIR_OUT & USB_ENDPOINT_DIR_MASK))
                        as u8,
                    bmAttributes: USB_ENDPOINT_XFER_INT as u8,
                    wMaxPacketSize: ((64 & USB_ENDPOINT_MAXP_MASK) as u16)
                        .to_le(),
                    bInterval: 255,
                    bRefresh: 0,
                    bSynchAddress: 0,
                },
            ],
            strings: vec![
                "string0",
                "Fakecompany",
                "Softproduct",
                "v0",
                "Default Config",
                "The Interface",
            ],
            parser: ctaphid::Parser::new(token, prompt),
        }
    }

    // pub fn submit(&mut self, urb: URB) -> R<bool> {
    //     Ok(true)
    // }
    pub fn init_callbacks(el: &mut eventloop::EventLoop<Device>) {
        let d2h = eventloop::Handler::Dev2Host(
            0,
            |el: &mut eventloop::EventLoop<Device>, mut urb| {
                let r = el
                    .state
                    .ep0_dev2host(urb.setup, &mut urb.transfer_buffer);
                urb.status = Some(match r {
                    Ok(()) => Ok(true),
                    Err(e) => Err(e.into()),
                });
                let complete = urb.complete.take().unwrap();
                complete(urb)
            },
        );
        el.schedule(d2h);
        let h2d = eventloop::Handler::Host2Dev(
            0,
            |el: &mut eventloop::EventLoop<Device>, mut urb| {
                let r = el.state.ep0_host2dev(urb.setup);
                urb.status = Some(match r {
                    Ok(()) => Ok(true),
                    Err(e) => Err(e.into()),
                });
                let complete = urb.complete.take().unwrap();
                complete(urb)
            },
        );
        el.schedule(h2d);
        let d2h1 = eventloop::Handler::Dev2Host(
            1,
            |el: &mut eventloop::EventLoop<Device>, mut urb| {
                let r = el.state.ep1_dev2host(&mut urb.transfer_buffer);
                urb.status = Some(match r {
                    Err(e) => Err(e.into()),
                    Ok(x) => Ok(x),
                });
                let complete = urb.complete.take().unwrap();
                complete(urb)
            },
        );
        el.schedule(d2h1);
        let h2d2 = eventloop::Handler::Host2Dev(
            2,
            |el: &mut eventloop::EventLoop<Device>, mut urb| {
                let r = el.state.ep2_host2dev(&urb.transfer_buffer);
                if !el.state.parser.recv_queue.is_empty()
                    || !el.state.parser.send_queue.is_empty()
                {
                    el.unblock_handler(1, true);
                };
                urb.status = Some(match r {
                    Err(e) => Err(e.into()),
                    Ok(x) => Ok(x),
                });
                let complete = urb.complete.take().unwrap();
                complete(urb);
            },
        );
        el.schedule(h2d2);
    }

    fn get_lang_descriptor(&self, sink: &mut dyn Write) -> IOR<()> {
        let d = usb_string_descriptor {
            bLength: size_of::<usb_string_descriptor>() as u8,
            bDescriptorType: DT::String.to_primitive(),
            wData: [LANG_ID_EN_US.to_le()],
        };
        write_struct(sink, &d)
    }

    fn get_string_descriptor(
        &self,
        index: u8,
        sink: &mut dyn Write,
    ) -> IOR<()> {
        assert!(index > 0);
        let text = self.strings[index as usize];
        let utf16_len = text.encode_utf16().count();
        let mut v = Vec::<u8>::with_capacity(utf16_len);
        text.encode_utf16().for_each(|u| {
            let bs = u.to_le_bytes();
            v.push(bs[0]);
            v.push(bs[1])
        });
        sink.write_all(&[
            2 + (utf16_len * 2) as u8,
            DT::String.to_primitive(),
        ])?;
        sink.write_all(&v)
    }

    fn get_descriptor(&self, req: SetupPacket, out: &mut [u8]) -> IOR<()> {
        let (value, lang, length) = req.args();
        let [index, ty] = value.to_le_bytes();
        let r#type = DT::from_primitive(ty).unwrap();
        println!(
            "GET_DESCRIPTOR: type: {:?} index: {} lang: {} length: {} ",
            r#type, index, lang, length
        );
        let sink = &mut std::io::Cursor::new(out);
        fn has_room(c: &std::io::Cursor<&mut [u8]>) -> bool {
            (c.position() as usize) < (c.get_ref().len() as usize)
        }
        use DescriptorType::*;
        match (r#type, index, lang) {
            (Device, 0, 0) => write_struct(sink, &self.device_descriptor),
            (Configuration, 0, 0) => {
                write_struct(sink, &self.config_descriptor)?;
                if has_room(sink) {
                    write_struct(sink, &self.interface_descriptor)?;
                    write_struct(sink, &self.hid_descriptor)?;
                    for epd in self.endpoint_descriptors.iter() {
                        let len = epd.bLength as usize;
                        write_struct_limited(sink, epd, len)?
                    }
                }
                Ok(())
            }
            (String, 0, 0) => self.get_lang_descriptor(sink),
            (String, i, LANG_ID_EN_US) => {
                self.get_string_descriptor(i, sink)
            }
            x => panic!("Unsupported descriptor: {:?}", x),
        }
    }

    fn get_interface_descriptor(
        &self,
        req: SetupPacket,
        mut out: &mut [u8],
    ) -> IOR<()> {
        let (value, _, _) = req.args();
        let [_, desctype] = value.to_le_bytes();
        match desctype as u32 {
            HID_DT_REPORT => out.write_all(&self.hid_report_descriptor),
            x => panic!("Unsupported descriptor type: {}", x),
        }
    }

    fn ep0_dev2host(&self, req: SetupPacket, sink: &mut [u8]) -> IOR<()> {
        match req.request_type() {
            (D2H, RT::Standard, RR::Device) => match req.std() {
                SR::GetDescriptor => self.get_descriptor(req, sink),
                SR::GetStatus if matches!(req.args(), (0, 0, 2)) => {
                    Ok(sink.copy_from_slice(&[1u8, 0]))
                }
                _ => unimplemented!(),
            },
            (D2H, RT::Standard, RR::Interface) => match req.std() {
                SR::GetDescriptor => {
                    self.get_interface_descriptor(req, sink)
                }
                _ => unimplemented!(),
            },
            x => panic!("Unsupported request: {:?}", x),
        }
    }

    fn ep0_host2dev(&self, req: SetupPacket) -> IOR<()> {
        match req.request_type() {
            (H2D, RT::Standard, RR::Device) => match req.std() {
                SR::SetConfiguration if req.args() == (0, 0, 0) => Ok(()),
                _ => unimplemented!(),
            },
            (H2D, RT::Class, RR::Interface) => match req.hid_request() {
                (HIDRequest::SetIdle, (0, 0, 0)) => Ok(()),
                _ => unimplemented!(),
            },
            _ => unimplemented!(),
        }
    }

    fn ep1_dev2host(&mut self, buf: &mut [u8]) -> R<bool> {
        log!("ep1 dev->host");
        while !self.parser.recv_queue.is_empty() {
            self.parser.parse()?
        }
        if !self.parser.send_queue.is_empty() {
            self.parser.unparse(buf)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn ep2_host2dev(&mut self, data: &[u8]) -> R<bool> {
        log!("ep2 host->dev");
        self.parser.recv_queue.push_back(data.to_vec());
        Ok(true)
    }
}

#[test]
fn test_get_device_descriptor() {
    let token = crate::crypto::tests::get_token().unwrap();
    let dev = Device::new(&token, &prompt::Pinentry {});
    let mut sink = [0u8; size_of::<usb_device_descriptor>()];
    const GET_DEVICE_DESCRIPTOR: &[u8; 8] =
        include_bytes!("../poke/get-device-descriptor.dat");
    let setup = SetupPacket::unpack(GET_DEVICE_DESCRIPTOR).unwrap();
    dev.ep0_dev2host(setup, &mut sink).unwrap();
    let d = crate::binio::test::view_as::<usb_device_descriptor>(&sink);
    assert_eq!(d.bLength, size_of::<usb_device_descriptor>() as u8);
    assert_eq!(d.bDescriptorType, DT::Device.to_primitive());
    assert_eq!(d.bDeviceClass, USB_CLASS_PER_INTERFACE as u8);
    assert_eq!(d.bNumConfigurations, 1);
    ()
}
