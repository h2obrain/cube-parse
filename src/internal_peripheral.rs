use std::error::Error;
use std::path::Path;
use std::collections::{HashMap,BTreeMap,BTreeSet};
use std::rc::Rc;

use lazy_static::lazy_static;
use regex::Regex;
use serde_derive::Deserialize;

use crate::utils::{load_file,ToPascalCase,SortedString,ToSortedString};

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

// TODO move GPIO_LETTER_REGEX/STEM_REGEX/AF_REGEX stuff here (see below)
//impl PinSignal {
//    fn get_af_value(&self) -> &str {
//        self.specific_parameter
//            .possible_value
//            .val
//            .split('_')
//            .collect::<Vec<_>>()[1]
//    }
//}

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

/// AfTree
///  TODO: replace tuple-types 
pub struct AfTree {
    mcu_gpio_map: AfTreeGpios,
    tree: AfTreeStems,
}
// stems
pub type AfTreeStems = BTreeMap<SortedString, AfTreeDevs>;
// devices (aka internal peripherals)
pub type AfTreeDevs = BTreeMap<SortedString, AfTreeIos>;
// device io, key:(af,io) value:(io-name,pin-map)
pub type AfTreeIos = BTreeMap<(SortedString,SortedString), (String,AfTreePins)>;
// pins, key:pin value:(pin-letter,pin-number,mcu-map)
pub type AfTreePins = BTreeMap<SortedString, (String,String,AfTreeGpios)>;
// gpios, key:gpio-mcu-name value:gpio-versions
pub type AfTreeGpios = BTreeMap<SortedString, AfTreeGpioVersions>;
// gpios, key:gpio-version value:mcus
pub type AfTreeGpioVersions = BTreeMap<SortedString, Rc<AfTreeMcus>>;
// mcus related to gpio
pub type AfTreeMcus = BTreeSet<SortedString>;

impl AfTree {
    pub fn new() -> Self {
        AfTree {
            mcu_gpio_map: AfTreeGpios::new(),
            tree: AfTreeStems::new()
        }
    }
    pub fn build(
        mcu_gpio_map: &HashMap<String, Vec<String>>,
        db_dir: &Path,
    ) -> Result<Self, String> {
        let mut af = AfTree::new();
    
        lazy_static! {
            static ref GPIO_REGEX: Regex = Regex::new(r#"^(?P<gpio>[a-zA-Z0-9]+)_(?P<version>gpio_\w+)$"#).unwrap();
            static ref MCUS_REGEX: Regex = Regex::new(r#"^(?P<mcu>STM32[A-Z]+[0-9]+)[A-Za-z][A-Za-z0-9]+$"#).unwrap();
        }
        
        for (gpio, mcus) in mcu_gpio_map {
            //println!("{:?}",gpio);
            // TODO filter out mcus here if needed..
            let gpio_data;
            match IpGPIO::load(db_dir, &gpio) {
                Ok(gd) => gpio_data = gd,
                Err(e) => {
                    eprintln!("Could not load IP GPIO file: {}", e);
                    continue; // warn only
                }
            }
            
            let gpio_mcu;
            let gpio_version;
            match GPIO_REGEX.captures(gpio) {
                Some(m) => {
                    gpio_mcu = m.name("gpio").unwrap().as_str();
                    gpio_version = m.name("version").unwrap().as_str();
                },
                None => {
                    eprintln!("FIXME: gpio-version '{}' could not be parsed to (gpio)_gpio_(version)! (ignoring)", gpio);
                    continue; // warn only
                }
            }
            
            let mut mcus_simplified: AfTreeMcus = AfTreeMcus::new();
            for mcu in mcus {
                match MCUS_REGEX.captures(mcu) {
                    Some(m) => {
                        mcus_simplified.insert(m.name("mcu").unwrap().as_str().to_lowercase().to_sorted_string());
                    },
                    None => {
                        eprintln!("FIXME: gpio-mcu '{}' could not be parsed to (STM32[LF..]xxx)YYY! (ignoring)", mcu);
                        continue; // warn only
                    }
                }
            }
            
            let mcus_simplified = Rc::new(mcus_simplified);
            
            let duplicated = af.mcu_gpio_map
                .entry(gpio_mcu.to_sorted_string()).or_insert_with(AfTreeGpioVersions::new)
                .insert(gpio_version.to_sorted_string(), mcus_simplified.clone());
            if duplicated.is_some() {
                eprintln!("FIXME: gpio '{}/{}' is duplicated! (ignoring)", gpio_mcu, gpio_version);
                // warn only
            }
            
            for p in gpio_data.gpio_pin {
                let name = p.get_name();
                if name.is_some() {
                    p.update_af_tree(&gpio_mcu, &gpio_version, mcus_simplified.clone(), &mut af.tree);
                }
            }
        }
        Ok(af)
    }
//    pub fn iter(&self, stem_selection: &Option<Vec<&str>>) -> Result<btree_map.Iter<SortedString, AfTreeDevs>, String> {
    pub fn iter(
        &self,
        stem_selection: &Option<Vec<&str>>,
    ) -> Result<impl Iterator<Item = (&SortedString, &AfTreeDevs)>, String>
    {
        // TODO ask someone how to do this correctly :)
        let sel: Vec<SortedString>;
        if let Some(stem_selection) = stem_selection {
            sel = stem_selection.iter().map(|m| m.to_sorted_string()).collect();
            // check selection
            let invalid_stems = sel.iter()
                .filter(|stem|{
                    !self.tree.contains_key(&stem)
                })
                .map(|stem| stem.to_string())
                .collect::<Vec<_>>();
            if !invalid_stems.is_empty() {
                return Err(format!("Invalid stem{} detected! ({})",
                    if invalid_stems.len() == 1 { "" } else { "s" },
                    invalid_stems.join("','")))
            };
        } else {
            sel = self.tree.keys().cloned().collect();
        }
        Ok(self.tree.iter().filter(move |(k,_v)| sel.contains(&k)))
    }
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

    pub fn update_af_tree(
        &self,
        gpio_mcu: &str,
        gpio_version: &str,
        mcus: Rc<AfTreeMcus>,
        af_tree: &mut AfTreeStems,
    ) {
        lazy_static! {
            static ref STEM_REGEX: Regex = Regex::new(
                r#"^(?P<dev>(?P<stem>((FMP)?I2|USB_OTG_)?[A-Z-]+)\d*(ext)?)(_(?P<io>[\w-]+))?$"#
            ).unwrap();
            static ref AF_REGEX: Regex = Regex::new(r#"^GPIO_(?P<af>[a-zA-Z\d]+)_\w+$"#).unwrap();
            static ref GPIO_LETTER_REGEX: Regex = Regex::new(r#"^P(?P<letter>[a-zA-Z]+)(?P<number>\d+)$"#).unwrap();
        }

        
        if let Some(ref v) = self.pin_signal {
            for sig in v {
                let m;
                match STEM_REGEX.captures(&sig.name) {
                    Some(m_) => m = m_,
                    None => {
                        eprintln!("FIXME: pin-signal '{}' could not be parsed! (ignoring)", sig.name);
                        continue;
                    }
                }
//                let af = sig.get_af_value().to_sorted_string();
                let af;
                match AF_REGEX.captures(&sig.specific_parameter.possible_value.val) {
                    Some(m) => af = m.name("af").unwrap().as_str().to_sorted_string(),
                    None => {
                        eprintln!(
                            "FIXME: af-pin-signal '{}' could not be parsed! (ignoring)",
                            sig.specific_parameter.possible_value.val
                        );
                        continue;
                    }
                }
                                
                let stem = m.name("stem").unwrap().as_str().to_sorted_string();
                let dev = m.name("dev").unwrap().as_str().to_sorted_string();
                let io = if let Some(io) = m.name("io") {
                        io.as_str().to_sorted_string()
                    } else {
                        // eventout and cec are ignored
                        if !["EVENTOUT","CEC"].contains(&stem.as_str()) {
                            eprintln!("FIXME: {} ({}) has no io part in its name! (assuming '{}')", stem, dev, stem);
                        }
                        stem.clone()
                    };
                let io_name = "Pin".to_string() + io.as_str().to_pascalcase().as_str();
                let pin = self.get_name().unwrap().to_sorted_string();
                
                // This is only needed for a new pin
                let pin_letter;
                let pin_number;
                match GPIO_LETTER_REGEX.captures(pin.as_str()) {
                    Some(m) => {
                        pin_letter = m.name("letter").unwrap().as_str().to_string();
                        pin_number = m.name("number").unwrap().as_str().to_string();
                    },
                    None => {
                        eprintln!("FIXME: pin '{}' could not be parsed to P(letter)(number)! (ignoring)", pin);
                        continue; // warn only
                    }
                }
                                
                let duplicated = af_tree
                    .entry(stem).or_insert_with(AfTreeDevs::new)
                    .entry(dev).or_insert_with(AfTreeIos::new)
                    .entry((af,io)).or_insert_with(||(io_name,AfTreePins::new())).1
                    .entry(pin).or_insert_with(||(pin_letter,pin_number,AfTreeGpios::new())).2
                    .entry(gpio_mcu.to_sorted_string()).or_insert_with(AfTreeGpioVersions::new)
                    .insert(gpio_version.to_sorted_string(), mcus.clone());
                
                if duplicated.is_some() {
                    eprintln!("FIXME: gpio '{}/{}' is duplicated! (ignoring)", gpio_mcu, gpio_version);
                    // warn only
                }
            }
        }
    }

}
