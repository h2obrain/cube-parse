use std::error::Error;
use std::path::Path;

use lazy_static::lazy_static;
use regex::Regex;
use serde_derive::Deserialize;

use crate::utils::load_file;

#[derive(Debug, Deserialize)]
pub(crate) struct PossibleValue {
    #[serde(rename = "$value")]
    pub(crate) val: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SpecificParameter {
    name: String,
    possible_value: PossibleValue,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PinSignal {
    name: String,
    specific_parameter: SpecificParameter,
}

impl PinSignal {
    fn get_af_value(&self) -> Result<u8, String> {
        let af_str = self
            .specific_parameter
            .possible_value
            .val
            .split('_')
            .collect::<Vec<_>>()[1];
        if &af_str[..2] != "AF" {
            return Err(format!("Invalid AF value: {}", af_str));
        }
        af_str[2..]
            .parse()
            .map_err(|e| format!("Could not parse AF value: {}", e))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename = "GPIO_Pin", rename_all = "PascalCase")]
pub struct GPIOPin {
    port_name: String,
    name: String,
    specific_parameter: Vec<SpecificParameter>,
    pin_signal: Option<Vec<PinSignal>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename = "IP")]
pub struct IpGPIO {
    #[serde(rename = "GPIO_Pin")]
    pub(crate) gpio_pin: Vec<GPIOPin>,
}

impl IpGPIO {
    pub fn load<P: AsRef<Path>>(db_dir: P, version: &str) -> Result<Self, Box<dyn Error>> {
        load_file(db_dir, format!("IP/GPIO-{}_Modes.xml", version))
    }
}

lazy_static! {
    static ref USART_RX: Regex = Regex::new("(US|LPU)ART._RX").unwrap();
    static ref USART_TX: Regex = Regex::new("(US|LPU)ART._TX").unwrap();
    static ref SPI_MOSI: Regex = Regex::new("SPI._MOSI").unwrap();
    static ref SPI_MISO: Regex = Regex::new("SPI._MISO").unwrap();
    static ref SPI_SCK: Regex = Regex::new("SPI._SCK").unwrap();
    static ref I2C_SCL: Regex = Regex::new("I2C._SCL").unwrap();
    static ref I2C_SDA: Regex = Regex::new("I2C._SDA").unwrap();
}

#[derive(Debug, Clone)]
pub struct AfSignal {
    pub signal_type: SignalType,
    pub peripheral: String,
    pub af: u8,
}

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum SignalType {
    Rx,
    Tx,
    Mosi,
    Miso,
    Sck,
    Sda,
    Scl,
}

impl GPIOPin {
    pub fn get_name(&self) -> Option<String> {
        let gpio_pin = self
            .specific_parameter
            .iter()
            .find(|v| v.name == "GPIO_Pin");
        match gpio_pin {
            Some(v) => {
                let num = v.possible_value.val.split('_').collect::<Vec<_>>()[2];
                Some(format!("{}{}", &self.port_name, num))
            }
            None => None,
        }
    }

    pub fn get_af_modes(&self) -> Result<Vec<AfSignal>, String> {
        let mut res: Vec<AfSignal> = vec![];
        if let Some(ref v) = self.pin_signal {
            for sig in v {
                let per = sig.name.split('_').collect::<Vec<_>>()[0];

                macro_rules! pin_signal {
                    ($type:expr) => {
                        res.push(AfSignal {
                            signal_type: $type,
                            peripheral: per.to_string(),
                            af: sig.get_af_value()?,
                        });
                    };
                }

                if USART_RX.is_match(&sig.name) {
                    pin_signal!(SignalType::Rx);
                }
                if USART_TX.is_match(&sig.name) {
                    pin_signal!(SignalType::Tx);
                }
                if SPI_MOSI.is_match(&sig.name) {
                    pin_signal!(SignalType::Mosi);
                }
                if SPI_MISO.is_match(&sig.name) {
                    pin_signal!(SignalType::Miso);
                }
                if SPI_SCK.is_match(&sig.name) {
                    pin_signal!(SignalType::Sck);
                }
                if I2C_SCL.is_match(&sig.name) {
                    pin_signal!(SignalType::Scl);
                }
                if I2C_SDA.is_match(&sig.name) {
                    pin_signal!(SignalType::Sda);
                }
            }
        }
        Ok(res)
    }
}
