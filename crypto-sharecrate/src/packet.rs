//! What goes *inside* an encrypted radio frame (the 36 plaintext bytes).
//!
//! Values are `i32` in "milli" units so no floats go over the air:
//! - `OneWireTemp`: milli-degrees C (23_500 = 23.5 °C)
//! - `AnalogIn`:    millivolts at the pin
//! - `DigitalIn` / `DigitalOut`: 0 or 1
//!
//! Unused bytes are zero padding, so every frame is the same size.

use crate::frame::{FrameType, RADIO_PLAINTEXT_LEN};
use crate::{Error, Result};

/// Max readings in one Data frame: 1 count byte + 7 * (channel u8 + value i32) = 36.
pub const MAX_READINGS: usize = 7;
/// Max channels in one Announce frame: 1 + 17 * 2 = 35.
pub const MAX_ANNOUNCE: usize = 17;
/// Max text bytes in a Message frame.
pub const MAX_TEXT: usize = RADIO_PLAINTEXT_LEN - 1;

/// What a channel (pin) on a node does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ChannelKind {
    /// Reads a pin high/low (button, float switch, PIR...).
    DigitalIn = 0,
    /// Drives a pin high/low (siren, solenoid, relay, pump...).
    DigitalOut = 1,
    /// Reads a voltage through the ADC (photoresistor, soil probe, pH board...).
    AnalogIn = 2,
    /// DS18B20 temperature on a 1-Wire bus.
    OneWireTemp = 3,
}

impl ChannelKind {
    pub fn from_byte(b: u8) -> Result<Self> {
        Ok(match b {
            0 => Self::DigitalIn,
            1 => Self::DigitalOut,
            2 => Self::AnalogIn,
            3 => Self::OneWireTemp,
            _ => return Err(Error::BadPacket),
        })
    }

    pub fn is_output(self) -> bool {
        matches!(self, Self::DigitalOut)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub channel: u8,
    pub value: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelInfo {
    pub channel: u8,
    pub kind: ChannelKind,
}

/// Decoded plaintext of an encrypted radio frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packet {
    Data {
        count: u8,
        readings: [Reading; MAX_READINGS],
    },
    Message {
        len: u8,
        text: [u8; MAX_TEXT],
    },
    Alert {
        channel: u8,
        code: u8,
        value: i32,
    },
    Command {
        channel: u8,
        value: i32,
    },
    Announce {
        count: u8,
        channels: [ChannelInfo; MAX_ANNOUNCE],
    },
    /// The node applied a Command: it echoes the channel and the value it set.
    Ack {
        channel: u8,
        value: i32,
    },
}

const EMPTY_READING: Reading = Reading {
    channel: 0,
    value: 0,
};
const EMPTY_INFO: ChannelInfo = ChannelInfo {
    channel: 0,
    kind: ChannelKind::DigitalIn,
};

impl Packet {
    pub fn data(readings: &[Reading]) -> Self {
        let n = readings.len().min(MAX_READINGS);
        let mut arr = [EMPTY_READING; MAX_READINGS];
        arr[..n].copy_from_slice(&readings[..n]);
        Packet::Data {
            count: n as u8,
            readings: arr,
        }
    }

    pub fn announce(channels: &[ChannelInfo]) -> Self {
        let n = channels.len().min(MAX_ANNOUNCE);
        let mut arr = [EMPTY_INFO; MAX_ANNOUNCE];
        arr[..n].copy_from_slice(&channels[..n]);
        Packet::Announce {
            count: n as u8,
            channels: arr,
        }
    }

    /// Text is cut to [`MAX_TEXT`] bytes (on a char boundary).
    pub fn message(s: &str) -> Self {
        let mut end = s.len().min(MAX_TEXT);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        let mut text = [0u8; MAX_TEXT];
        text[..end].copy_from_slice(&s.as_bytes()[..end]);
        Packet::Message {
            len: end as u8,
            text,
        }
    }

    pub fn readings(&self) -> &[Reading] {
        match self {
            Packet::Data { count, readings } => &readings[..*count as usize],
            _ => &[],
        }
    }

    pub fn channels(&self) -> &[ChannelInfo] {
        match self {
            Packet::Announce { count, channels } => &channels[..*count as usize],
            _ => &[],
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Packet::Message { len, text } => {
                core::str::from_utf8(&text[..*len as usize]).unwrap_or("<bad utf8>")
            }
            _ => "",
        }
    }

    pub fn frame_type(&self) -> FrameType {
        match self {
            Packet::Data { .. } => FrameType::Data,
            Packet::Message { .. } => FrameType::Message,
            Packet::Alert { .. } => FrameType::Alert,
            Packet::Command { .. } => FrameType::Command,
            Packet::Announce { .. } => FrameType::Announce,
            Packet::Ack { .. } => FrameType::Ack,
        }
    }

    pub fn encode(&self) -> [u8; RADIO_PLAINTEXT_LEN] {
        let mut b = [0u8; RADIO_PLAINTEXT_LEN];
        match self {
            Packet::Data { count, readings } => {
                b[0] = *count;
                for (i, r) in readings[..*count as usize].iter().enumerate() {
                    let o = 1 + i * 5;
                    b[o] = r.channel;
                    b[o + 1..o + 5].copy_from_slice(&r.value.to_le_bytes());
                }
            }
            Packet::Message { len, text } => {
                b[0] = *len;
                b[1..1 + *len as usize].copy_from_slice(&text[..*len as usize]);
            }
            Packet::Alert {
                channel,
                code,
                value,
            } => {
                b[0] = *channel;
                b[1] = *code;
                b[2..6].copy_from_slice(&value.to_le_bytes());
            }
            Packet::Command { channel, value } => {
                b[0] = *channel;
                b[1..5].copy_from_slice(&value.to_le_bytes());
            }
            Packet::Announce { count, channels } => {
                b[0] = *count;
                for (i, c) in channels[..*count as usize].iter().enumerate() {
                    b[1 + i * 2] = c.channel;
                    b[2 + i * 2] = c.kind as u8;
                }
            }
            Packet::Ack { channel, value } => {
                b[0] = *channel;
                b[1..5].copy_from_slice(&value.to_le_bytes());
            }
        }
        b
    }

    pub fn decode(ftype: FrameType, b: &[u8]) -> Result<Self> {
        if b.len() != RADIO_PLAINTEXT_LEN {
            return Err(Error::BadPacket);
        }
        let i32_at = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        Ok(match ftype {
            FrameType::Data => {
                let count = b[0];
                if count as usize > MAX_READINGS {
                    return Err(Error::BadPacket);
                }
                let mut readings = [EMPTY_READING; MAX_READINGS];
                for (i, r) in readings.iter_mut().take(count as usize).enumerate() {
                    let o = 1 + i * 5;
                    *r = Reading {
                        channel: b[o],
                        value: i32_at(o + 1),
                    };
                }
                Packet::Data { count, readings }
            }
            FrameType::Message => {
                let len = b[0];
                if len as usize > MAX_TEXT {
                    return Err(Error::BadPacket);
                }
                let mut text = [0u8; MAX_TEXT];
                text.copy_from_slice(&b[1..]);
                Packet::Message { len, text }
            }
            FrameType::Alert => Packet::Alert {
                channel: b[0],
                code: b[1],
                value: i32_at(2),
            },
            FrameType::Command => Packet::Command {
                channel: b[0],
                value: i32_at(1),
            },
            FrameType::Announce => {
                let count = b[0];
                if count as usize > MAX_ANNOUNCE {
                    return Err(Error::BadPacket);
                }
                let mut channels = [EMPTY_INFO; MAX_ANNOUNCE];
                for (i, c) in channels.iter_mut().take(count as usize).enumerate() {
                    *c = ChannelInfo {
                        channel: b[1 + i * 2],
                        kind: ChannelKind::from_byte(b[2 + i * 2])?,
                    };
                }
                Packet::Announce { count, channels }
            }
            FrameType::Ack => Packet::Ack {
                channel: b[0],
                value: i32_at(1),
            },
            _ => return Err(Error::BadPacket),
        })
    }
}

/// Alert codes a node can raise.
pub mod alert_code {
    pub const SENSOR_FAULT: u8 = 1;
    pub const THRESHOLD: u8 = 2;
    pub const REBOOTED: u8 = 3;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_roundtrip() {
        let p = Packet::data(&[
            Reading {
                channel: 0,
                value: 23_500,
            },
            Reading {
                channel: 1,
                value: -40,
            },
        ]);
        let d = Packet::decode(FrameType::Data, &p.encode()).unwrap();
        assert_eq!(d, p);
        assert_eq!(d.readings().len(), 2);
    }

    #[test]
    fn announce_and_message_roundtrip() {
        let p = Packet::announce(&[
            ChannelInfo {
                channel: 0,
                kind: ChannelKind::OneWireTemp,
            },
            ChannelInfo {
                channel: 3,
                kind: ChannelKind::DigitalOut,
            },
        ]);
        assert_eq!(Packet::decode(FrameType::Announce, &p.encode()).unwrap(), p);
        let m = Packet::message("hello from node 1");
        assert_eq!(
            Packet::decode(FrameType::Message, &m.encode())
                .unwrap()
                .text(),
            "hello from node 1"
        );
    }

    #[test]
    fn bad_counts_rejected() {
        let mut b = [0u8; RADIO_PLAINTEXT_LEN];
        b[0] = 200;
        assert!(Packet::decode(FrameType::Data, &b).is_err());
        assert!(Packet::decode(FrameType::Announce, &b).is_err());
    }
}
