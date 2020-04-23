use std::error::Error;
use std::path::Path;
use std::collections::{HashMap,BTreeMap,BTreeSet};
use std::rc::Rc;

use lazy_static::lazy_static;
use regex::Regex;
use serde_derive::Deserialize;

use crate::utils::{load_file,ToPascalCase,SortedString,ToSortedString};


// TODO f1 compatibility with something like that
//  if f1 {
//    if SpecificParameter not present {
//      manually add SpecificParameter.name=GPIO_AF
//      SpecificParameter.PossibleValue = GPIO_AF_DISABLE
//    } else {
//      SpecificParameter.PossibleValue = GPIO_AF_ENABLE
//    }
//  }

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
    mcu_gpio_map: AfTreeGpiosAll,
    tree: AfTreeStems,
}
// stems
pub type AfTreeStems = BTreeMap<SortedString, AfTreeDevs>;
// devices (aka internal peripherals)
pub type AfTreeDevs = BTreeMap<SortedString, AfTreeIos>;
// device io, key:(af,io) value:(io-name,pin-map)
pub type AfTreeIos = BTreeMap<(SortedString,SortedString), (String,AfTreePins)>;
// pins, key:(port-name,pin-number) value:(original-names,mcu-map)
pub type AfTreePins = BTreeMap<(String,u32), (BTreeSet<SortedString>,AfTreeGpios)>;
// gpios, key:gpio-mcu-name value:gpio-versions
pub type AfTreeGpiosAll = BTreeMap<SortedString, (String,AfTreeGpioVersions)>;
pub type AfTreeGpios = BTreeMap<SortedString, AfTreeGpioVersions>;
// gpios, key:gpio-version value:mcus
pub type AfTreeGpioVersions = BTreeMap<SortedString, Rc<AfTreeMcus>>;
// mcus related to gpio
pub type AfTreeMcus = BTreeSet<SortedString>;

impl AfTree {
    pub fn new() -> Self {
        AfTree {
            mcu_gpio_map: AfTreeGpiosAll::new(),
            tree: AfTreeStems::new()
        }
    }
    pub fn build(
        family: &str,
        mcu_gpio_map: &HashMap<String, Vec<String>>,
        db_dir: &Path,
        simplify_mcu_names: bool,
    ) -> Result<Self, String> {
        if family=="STM32F1" {
            return Err("The STM32F1-family is unforunately not supported! (see \"TODO f1 compatibility\" in internal_peripherals.rs)".to_string());
        }
        
        let mut af = AfTree::new();
    
        lazy_static! {
            static ref GPIO_REGEX: Regex = Regex::new(r#"^(?P<gpio>[a-zA-Z0-9]+)_(?P<version>gpio_\w+)$"#).unwrap();
            static ref MCUS_REGEX: Regex = Regex::new(r#"^(?P<mcu>STM32[A-Z]+[0-9]+)[A-Za-z][A-Za-z0-9]+$"#).unwrap();
        }
        
        for (gpio, mcus) in mcu_gpio_map {
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
            eprintln!("{} => {}/{}", gpio,gpio_mcu,gpio_version);
            
            let mut mcus_simplified: AfTreeMcus = AfTreeMcus::new();
            if simplify_mcu_names {
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
            } else {
                mcus_simplified = mcus.iter().map(|mcu|mcu.to_sorted_string()).collect();
            }
            
            let mcus_simplified = Rc::new(mcus_simplified);
            
            for p in gpio_data.gpio_pin {
                p.update_af_tree(&gpio_mcu, &gpio_version, &mcus_simplified, &mut af.tree);
            }

            if let Some(duplicated) = af.mcu_gpio_map
                .entry(gpio_mcu.to_sorted_string()).or_insert_with(||(gpio.to_string(),AfTreeGpioVersions::new())).1
                .insert(gpio_version.to_sorted_string(), mcus_simplified) //.clone()
            {
                let harmless = duplicated == af.mcu_gpio_map[&gpio_mcu.to_sorted_string()].1[&gpio_version.to_sorted_string()];
                eprintln!("FIXME: gpio '{}=>{} ({}/{})' is duplicated{}! (ignoring)",
                    gpio_mcu, gpio_version,
                    af.mcu_gpio_map[&gpio_mcu.to_sorted_string()].0, // A
                    gpio, // B
                    if harmless {" in a harmless way"} else {""}
                );
                // warn only
            }
        }
        Ok(af)
    }
    pub fn iter(
        &self,
        stem_selection: &Option<Vec<&str>>,
    ) -> Result<impl Iterator<Item = (&SortedString, &AfTreeDevs)>, String> {
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
    /// Build name from self.port_name and GPIO_PIN_##
    #[allow(dead_code)]
    pub fn get_name(&self) -> Option<String> {
        match self.get_pin_nr() {
            Some(num) => Some(format!("{}{}", &self.port_name, num)),
            None => None,
        }
    }
    
    /// Build pin_nr from GPIO_PIN_## instead of
    /// using self.name which can have strange values like
    /// "PA10 [PA12]" for a remappable pin
    pub fn get_pin_nr(&self) -> Option<u32> {
        match self
            .specific_parameter
            .iter()
            .find(|v| v.name == "GPIO_Pin") {
            Some(v) => v.possible_value.val.split('_').collect::<Vec<_>>()[2].parse().map_or(None,Some),
            None => None,
        }
    }

    pub fn update_af_tree(
        &self,
        gpio_mcu: &str,
        gpio_version: &str,
        mcus: &Rc<AfTreeMcus>,
        af_tree: &mut AfTreeStems
    ) {
        lazy_static! {
            static ref STEM_REGEX: Regex = Regex::new(
                r#"^(?P<dev>(?P<stem>((FMP)?I2|USB_OTG_)?[A-Z-]+)\d*(ext)?)(_(?P<io>[\w-]+))?$"#
            ).unwrap();
            static ref AF_REGEX: Regex = Regex::new(r#"^GPIO_(?P<af>[a-zA-Z\d]+)_\w+$"#).unwrap();
        }

        // try to get pin_nr
        let pin_nr;
        match self.get_pin_nr() {
            Some(nr) => pin_nr = nr,
            None => {
                eprintln!("FIXME: pin with name '{}' has no decimal number in its SpecificParameter(PinName).PossibleValue-field! (ignoring)", self.name);
                return;
            }
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
                
                // do not allow duplicated (would also be enough..)
                let pin = af_tree
                    .entry(stem.clone()).or_insert_with(AfTreeDevs::new)
                    .entry(dev.clone()).or_insert_with(AfTreeIos::new)
                    .entry((af.clone(),io.clone())).or_insert_with(||(io_name,AfTreePins::new())).1
                    .entry((self.port_name.to_string(),pin_nr)).or_insert_with(||(BTreeSet::new(),AfTreeGpios::new()));
                pin.0.insert(self.name.to_sorted_string());
                let duplicated = pin.1
                    .entry(gpio_mcu.to_sorted_string()).or_insert_with(AfTreeGpioVersions::new)
                    .insert(gpio_version.to_sorted_string(), mcus.clone());
                // error
                if let Some(_duplicated) = duplicated {
                    eprintln!("FIXME: gpio '{}=>{}=>{}=>{}=>{}=>{}' is duplicated! (pin-names:{:?}) (ignoring)",
                    stem,dev,af,io,gpio_mcu,gpio_version,
                    pin.0);
                    // do nothing
                }
            }
        }
    }
}
