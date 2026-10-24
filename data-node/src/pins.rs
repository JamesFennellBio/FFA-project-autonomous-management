//! Generic channel table: any pin can be a digital input, digital output,
//! analog input (ADC1) or DS18B20 thermometer. Add channels in
//! `main.rs::build_channels`.

use alloc::boxed::Box;

use crypto_sharecrate::packet::{ChannelInfo, ChannelKind, MAX_READINGS, Reading};
use embassy_time::Timer;
use esp_hal::Blocking;
use esp_hal::analog::adc::{Adc, AdcCalScheme, AdcChannel, AdcPin};
use esp_hal::gpio::{
    Input, InputConfig, InputPin, Level, Output, OutputConfig, OutputPin, Pin, Pull,
};
use esp_hal::peripherals::ADC1;
use heapless::Vec;

use crate::onewire::Ds18b20;

type Adc1<'d> = Adc<'d, ADC1<'d>, Blocking>;
type AnalogReader<'d> = Box<dyn FnMut(&mut Adc1<'d>) -> Option<u16> + 'd>;

#[allow(dead_code)] // DigitalIn is for your own sensors (see main.rs)
enum Hw<'d> {
    DigitalIn(Input<'d>),
    DigitalOut(Output<'d>),
    Analog(AnalogReader<'d>),
    Temp(Ds18b20<'d>),
}

pub struct Channels<'d> {
    adc: Option<Adc1<'d>>,
    list: Vec<(u8, Hw<'d>), MAX_READINGS>,
}

impl<'d> Channels<'d> {
    pub fn new(adc: Option<Adc1<'d>>) -> Self {
        Self {
            adc,
            list: Vec::new(),
        }
    }

    fn push(&mut self, ch: u8, hw: Hw<'d>) {
        if self.list.push((ch, hw)).is_err() {
            log::warn!("too many channels; channel {ch} ignored (max {MAX_READINGS})");
        }
    }

    /// Button, float switch, PIR... reads 1 when the pin is high.
    #[allow(dead_code)]
    pub fn digital_in(&mut self, ch: u8, pin: impl InputPin + 'd, pull: Pull) {
        self.push(
            ch,
            Hw::DigitalIn(Input::new(pin, InputConfig::default().with_pull(pull))),
        );
    }

    /// Siren, solenoid, relay... starts OFF (low).
    pub fn digital_out(&mut self, ch: u8, pin: impl OutputPin + 'd) {
        self.push(
            ch,
            Hw::DigitalOut(Output::new(pin, Level::Low, OutputConfig::default())),
        );
    }

    /// An ADC1 pin already set up with `AdcConfig::enable_pin_with_cal`.
    /// Reports millivolts.
    pub fn analog_in<P, CS>(&mut self, ch: u8, mut pin: AdcPin<P, ADC1<'d>, CS>)
    where
        P: AdcChannel + 'd,
        CS: AdcCalScheme<ADC1<'d>> + 'd,
    {
        self.push(
            ch,
            Hw::Analog(Box::new(move |adc: &mut Adc1<'d>| {
                nb::block!(adc.read_oneshot(&mut pin)).ok()
            })),
        );
    }

    pub fn ds18b20(&mut self, ch: u8, pin: impl Pin + 'd) {
        self.push(ch, Hw::Temp(Ds18b20::new(pin)));
    }

    /// Channel list for the Announce packet.
    pub fn info(&self) -> Vec<ChannelInfo, MAX_READINGS> {
        self.list
            .iter()
            .map(|(ch, hw)| ChannelInfo {
                channel: *ch,
                kind: match hw {
                    Hw::DigitalIn(_) => ChannelKind::DigitalIn,
                    Hw::DigitalOut(_) => ChannelKind::DigitalOut,
                    Hw::Analog(_) => ChannelKind::AnalogIn,
                    Hw::Temp(_) => ChannelKind::OneWireTemp,
                },
            })
            .collect()
    }

    /// Reads every channel (outputs report their current state). Returns the
    /// readings plus the channels whose sensor didn't answer.
    pub async fn sample(&mut self) -> (Vec<Reading, MAX_READINGS>, Vec<u8, MAX_READINGS>) {
        let mut any_temp = false;
        for (_, hw) in self.list.iter_mut() {
            if let Hw::Temp(t) = hw {
                any_temp |= t.start_conversion();
            }
        }
        if any_temp {
            Timer::after_millis(750).await; // DS18B20 12-bit conversion time
        }
        let mut out = Vec::new();
        let mut faults = Vec::new();
        for (ch, hw) in self.list.iter_mut() {
            let value = match hw {
                Hw::DigitalIn(i) => Some(i.is_high() as i32),
                Hw::DigitalOut(o) => Some(o.is_set_high() as i32),
                Hw::Analog(read) => self
                    .adc
                    .as_mut()
                    .and_then(|adc| read(adc))
                    .map(|mv| mv as i32),
                Hw::Temp(t) => t.read_milli_c(),
            };
            match value {
                Some(v) => {
                    let _ = out.push(Reading {
                        channel: *ch,
                        value: v,
                    });
                }
                None => {
                    let _ = faults.push(*ch);
                }
            }
        }
        (out, faults)
    }

    /// Sets an output. Returns false if `ch` isn't an output.
    pub fn set_output(&mut self, ch: u8, value: i32) -> bool {
        for (c, hw) in self.list.iter_mut() {
            if *c == ch {
                if let Hw::DigitalOut(o) = hw {
                    o.set_level(if value != 0 { Level::High } else { Level::Low });
                    return true;
                }
            }
        }
        false
    }

    /// Turns every output off (used when contact with the hub is lost).
    pub fn all_outputs_off(&mut self) {
        for (_, hw) in self.list.iter_mut() {
            if let Hw::DigitalOut(o) = hw {
                o.set_low();
            }
        }
    }
}
