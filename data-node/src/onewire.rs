//! Minimal bit-banged 1-Wire driver for one DS18B20 per pin.
//!
//! Wiring: DS18B20 data -> GPIO, 4.7 kΩ pull-up from data to 3V3, GND, VDD 3V3.
//! (The internal pull-up is far too weak for 1-Wire on its own.)

use esp_hal::delay::Delay;
use esp_hal::gpio::{DriveMode, Flex, OutputConfig, Pin, Pull};

pub struct Ds18b20<'d> {
    pin: Flex<'d>,
    delay: Delay,
}

impl<'d> Ds18b20<'d> {
    pub fn new(pin: impl Pin + 'd) -> Self {
        let mut pin = Flex::new(pin);
        pin.apply_output_config(
            &OutputConfig::default()
                .with_drive_mode(DriveMode::OpenDrain)
                .with_pull(Pull::Up),
        );
        pin.set_high(); // released (open drain: high = let the pull-up win)
        pin.set_output_enable(true);
        pin.set_input_enable(true);
        Self {
            pin,
            delay: Delay::new(),
        }
    }

    /// Reset pulse. Returns true if a sensor answered with a presence pulse.
    fn reset(&mut self) -> bool {
        critical_section::with(|_| {
            self.pin.set_low();
            self.delay.delay_micros(480);
            self.pin.set_high();
            self.delay.delay_micros(70);
            let present = self.pin.is_low();
            self.delay.delay_micros(410);
            present
        })
    }

    fn write_bit(&mut self, bit: bool) {
        critical_section::with(|_| {
            self.pin.set_low();
            if bit {
                self.delay.delay_micros(6);
                self.pin.set_high();
                self.delay.delay_micros(64);
            } else {
                self.delay.delay_micros(60);
                self.pin.set_high();
                self.delay.delay_micros(10);
            }
        })
    }

    fn read_bit(&mut self) -> bool {
        critical_section::with(|_| {
            self.pin.set_low();
            self.delay.delay_micros(3);
            self.pin.set_high();
            self.delay.delay_micros(10);
            let b = self.pin.is_high();
            self.delay.delay_micros(53);
            b
        })
    }

    fn write_byte(&mut self, mut v: u8) {
        for _ in 0..8 {
            self.write_bit(v & 1 == 1);
            v >>= 1;
        }
    }

    fn read_byte(&mut self) -> u8 {
        let mut v = 0u8;
        for i in 0..8 {
            if self.read_bit() {
                v |= 1 << i;
            }
        }
        v
    }

    /// Starts a temperature conversion (takes up to 750 ms at 12-bit).
    pub fn start_conversion(&mut self) -> bool {
        if !self.reset() {
            return false;
        }
        self.write_byte(0xCC); // SKIP ROM (only one sensor on this pin)
        self.write_byte(0x44); // CONVERT T
        true
    }

    /// Reads the last conversion in milli-degrees C. `None` if there's no
    /// sensor or the CRC is bad.
    pub fn read_milli_c(&mut self) -> Option<i32> {
        if !self.reset() {
            return None;
        }
        self.write_byte(0xCC);
        self.write_byte(0xBE); // READ SCRATCHPAD
        let mut s = [0u8; 9];
        for b in s.iter_mut() {
            *b = self.read_byte();
        }
        if crc8(&s[..8]) != s[8] || s.iter().all(|&b| b == 0xFF) {
            return None;
        }
        let raw = i16::from_le_bytes([s[0], s[1]]) as i32; // 1/16 °C per bit
        Some(raw * 125 / 2) // raw * 62.5 = milli-degrees
    }
}

/// Dallas/Maxim CRC-8 (polynomial x^8 + x^5 + x^4 + 1, reflected).
fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in data {
        let mut b = byte;
        for _ in 0..8 {
            let mix = (crc ^ b) & 1;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0x8C;
            }
            b >>= 1;
        }
    }
    crc
}
