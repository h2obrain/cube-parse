use std::{collections::HashMap, env, path::Path};

use alphanumeric_sort::{compare_str,sort_str_slice};
use clap::{App, Arg};
use lazy_static::lazy_static;
use regex::Regex;

mod family;
mod internal_peripheral;
mod mcu;
mod utils;

#[derive(Debug, PartialEq)]
enum GenerateTarget {
    QueryPinMappings,
    PinMappings,
    Features,
}

lazy_static! {
    // Note: Version >1.0 is not currently supported
    static ref GPIO_VERSION: Regex = Regex::new("^([^_]*)_gpio_v1_0$").unwrap();
}

/// Convert a GPIO IP version (e.g. "STM32L152x8_gpio_v1_0") to a feature name
/// (e.g. "io-STM32L152x8").
fn gpio_version_to_feature(version: &str) -> Result<String, String> {
    if let Some(captures) = GPIO_VERSION.captures(version) {
        Ok(format!("io-{}", captures.get(1).unwrap().as_str()))
    } else {
        Err(format!("Could not parse version {:?}", version))
    }
}

fn main() -> Result<(), String> {
    let args = App::new("cube-parse")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Extract AF modes on MCU pins from the database files provided with STM32CubeMX")
        .author(&*env!("CARGO_PKG_AUTHORS").replace(":", ", "))
        .arg(
            Arg::with_name("db_dir")
                .short("d")
                .help("Path to the CubeMX MCU database directory")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name("generate")
                .help("What to generate")
                .takes_value(true)
                .possible_values(&["pin_mappings", "features", "query"])
                .required(true),
        )
        .arg(
            Arg::with_name("mcu_family")
                .help("The MCU family to extract, e.g. \"STM32L0\"")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name("mcu")
                .short("m")
                .takes_value(true)
                .help("The (partial) mcu-definition, e.g. \"STM32F429\"")
                .required(false),
        )
        .arg(
            Arg::with_name("af_stems")
                .short("f")
                .takes_value(true)
                .help("The STEM of the pin-af, e.g. \"TIM\" or \"FMC\"")
                .multiple(true)
                .required(false),
        )
        .get_matches();

    // Process args
    let db_dir = Path::new(args.value_of("db_dir").unwrap());
    let mcu_family = args.value_of("mcu_family").unwrap();
    let generate = match args.value_of("generate").unwrap() {
        "query" => GenerateTarget::QueryPinMappings,
        "pin_mappings" => GenerateTarget::PinMappings,
        "features" => GenerateTarget::Features,
        _ => unreachable!(),
    };
    let af_stems = match args.values_of("af_stems") {
        Some(af_stems) => Some(af_stems.collect()),
        None => None,
    };
    //println!("stems: {:?}", af_stems);

    // Load families
    let families = family::Families::load(&db_dir)
        .map_err(|e| format!("Could not load families XML: {}", e))?;

    // Find target family
    let family = (&families)
        .into_iter()
        .find(|v| v.name == mcu_family)
        .ok_or_else(|| format!("Could not find family {}", mcu_family))?;

    // MCU map
    //
    // The keys of this map are GPIO peripheral version strings (e.g.
    // "STM32L051_gpio_v1_0"), while the value is a Vec of MCU ref names.
    let mut mcu_gpio_map: HashMap<String, Vec<String>> = HashMap::new();

    // Package map
    //
    // The keys of this map are MCU ref names, the values are package names
    // (e.g. ).
    let mut mcu_package_map: HashMap<String, String> = HashMap::new();

    for sf in family {
        for mcu in sf {
            let mcu_dat = mcu::Mcu::load(&db_dir, &mcu.name)
                .map_err(|e| format!("Could not load MCU data: {}", e))?;

            let gpio_version = mcu_dat.get_ip("GPIO").unwrap().get_version().to_string();
            mcu_gpio_map
                .entry(gpio_version)
                .or_insert(vec![])
                .push(mcu.ref_name.clone());

            if mcu_family == "STM32L0" {
                // The stm32l0xx-hal has package based features
                mcu_package_map.insert(mcu.ref_name.clone(), mcu.package_name.clone());
            }
        }
    }

    match generate {
        GenerateTarget::Features => {
            generate_features(&mcu_gpio_map, &mcu_package_map, &mcu_family)?
        }
        GenerateTarget::PinMappings => generate_pin_mappings(&mcu_gpio_map, &db_dir)?,
        GenerateTarget::QueryPinMappings => query_pin_mappings(&mcu_gpio_map, &db_dir, &af_stems)?,
    };

    Ok(())
}

lazy_static! {
    static ref FEATURE_DEPENDENCIES: HashMap<&'static str, HashMap<&'static str, &'static str>> = {
        let mut m = HashMap::new();

        // STM32L0
        let mut l0 = HashMap::new();
        l0.insert("^STM32L0.1", "stm32l0x1");
        l0.insert("^STM32L0.2", "stm32l0x2");
        l0.insert("^STM32L0.3", "stm32l0x3");
        m.insert("STM32L0", l0);

        m
    };
}

/// Print the IO features, followed by MCU features that act purely as aliases
/// for the IO features.
///
/// Both lists are sorted alphanumerically.
fn generate_features(
    mcu_gpio_map: &HashMap<String, Vec<String>>,
    mcu_package_map: &HashMap<String, String>,
    mcu_family: &str,
) -> Result<(), String> {
    let mut main_features = mcu_gpio_map
        .keys()
        .map(|gpio| gpio_version_to_feature(gpio))
        .collect::<Result<Vec<String>, String>>()?;
    main_features.sort();

    let mut mcu_aliases = vec![];
    for (gpio, mcu_list) in mcu_gpio_map {
        let gpio_version_feature = gpio_version_to_feature(gpio).unwrap();
        for mcu in mcu_list {
            let mut dependencies = vec![];

            // GPIO version feature
            dependencies.push(gpio_version_feature.clone());

            // Additional dependencies
            if let Some(family) = FEATURE_DEPENDENCIES.get(mcu_family) {
                for (pattern, feature) in family {
                    if Regex::new(pattern).unwrap().is_match(&mcu) {
                        dependencies.push(feature.to_string());
                        break;
                    }
                }
            }

            // Package based feature
            if let Some(package) = mcu_package_map.get(mcu) {
                dependencies.push(package.to_lowercase());
            }

            let mcu_feature = format!("mcu-{}", mcu);
            mcu_aliases.push(format!(
                "{} = [{}]",
                mcu_feature,
                &dependencies.iter().map(|val| format!("\"{}\"", val)).fold(
                    String::new(),
                    |mut acc, x| {
                        if !acc.is_empty() {
                            acc.push_str(", ");
                        }
                        acc.push_str(&x);
                        acc
                    }
                )
            ));
        }
    }
    mcu_aliases.sort();

    println!("# Features based on the GPIO peripheral version");
    println!("# This determines the pin function mapping of the MCU");
    for feature in main_features {
        println!("{} = []", feature);
    }
    println!();
    if !mcu_package_map.is_empty() {
        println!("# Physical packages");
        let mut packages = mcu_package_map
            .values()
            .map(|v| v.to_lowercase())
            .collect::<Vec<_>>();
        packages.sort_by(|a, b| compare_str(a, b));
        packages.dedup();
        for pkg in packages {
            println!("{} = []", pkg);
        }
        println!();
    }
    println!("# MCUs");
    for alias in mcu_aliases {
        println!("{}", alias);
    }

    Ok(())
}

/// Generate the pin mappings for the target MCU family.
fn generate_pin_mappings(
    mcu_gpio_map: &HashMap<String, Vec<String>>,
    db_dir: &Path,
) -> Result<(), String> {
    let mut gpio_versions = mcu_gpio_map.keys().collect::<Vec<_>>();
    gpio_versions.sort();
    for gpio in gpio_versions {
        let gpio_version_feature = gpio_version_to_feature(&gpio)?;
        println!("#[cfg(feature = \"{}\")]", gpio_version_feature);
        let gpio_data = internal_peripheral::IpGPIO::load(db_dir, &gpio)
            .map_err(|e| format!("Could not load IP GPIO file: {}", e))?;
        render_pin_modes(&gpio_data);
        println!("\n");
    }
    Ok(())
}

fn render_pin_modes(ip: &internal_peripheral::IpGPIO) {
    let mut pin_map: HashMap<String, Vec<String>> = HashMap::new();

    for p in &ip.gpio_pin {
        let name = p.get_name();
        if let Some(n) = name {
            pin_map.insert(n, p.get_af_modes());
        }
    }

    let mut pin_map = pin_map
        .into_iter()
        .map(|(k, mut v)| {
            #[allow(clippy::redundant_closure)]
            v.sort_by(|a, b| compare_str(a, b));
            (k, v)
        })
        .collect::<Vec<_>>();

    pin_map.sort_by(|a, b| compare_str(&a.0, &b.0));

    println!("pins! {{");
    for (n, af) in pin_map {
        if af.is_empty() {
            continue;
        } else if af.len() == 1 {
            println!("    {} => {{{}}},", n, af[0]);
        } else {
            println!("    {} => {{", n);
            for a in af {
                println!("        {},", a);
            }
            println!("    }},");
        }
    }
    println!("}}");
}

/// Query the pin mappings for the target MCU family.
fn query_pin_mappings(
    mcu_gpio_map: &HashMap<String, Vec<String>>,
    db_dir: &Path,
    af_stem_selection: &Option<Vec<&str>>,
) -> Result<(), String> {
    let mut af_tree = HashMap::new();
    
    let mut gpio_versions = mcu_gpio_map.keys().collect::<Vec<_>>();
    gpio_versions.sort();
    for gpio in gpio_versions {
        //println!("{:?}",gpio);
        // TODO filter out mcus here if needed..
        let gpio_data = internal_peripheral::IpGPIO::load(db_dir, &gpio)
            .map_err(|e| format!("Could not load IP GPIO file: {}", e))?;
        
        for p in gpio_data.gpio_pin {
            let name = p.get_name();
            if name.is_some() {
                p.update_af_tree(gpio, &mut af_tree);
            }
        }
    }
    
//    println!("All stems");
    let mut af_stems = if let Some(af_stem_selection) = af_stem_selection {
        af_stem_selection.iter()
            .map(|m| {
                let m = (*m).to_string();
                if af_tree.contains_key(&m) { Ok(m) } else { Err("Invalid stem detected!") }
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        af_tree.keys().cloned().collect::<Vec<_>>()
    };
    sort_str_slice(&mut af_stems);
    for af_stem in af_stems {
        let mut afs = af_tree[&af_stem].keys().cloned().collect::<Vec<_>>();
        sort_str_slice(&mut afs);
        println!("{}", af_stem);
        //println!("{} ({})", af_stem, afs.join(", "));
        for af in afs {
            println!("  {}", af);
            let mut af_pins = af_tree[&af_stem][&af].keys().cloned().collect::<Vec<_>>();
            sort_str_slice(&mut af_pins);
            for af_pin in af_pins {
                let af_pin_name = convert_to_camel_case(af_pin.as_str()) + "Pin";
                let mut pins = af_tree[&af_stem][&af][&af_pin].keys().cloned().collect::<Vec<_>>();
                sort_str_slice(&mut pins);
                let pin_names = pins.iter().map(|k|format!("{:4}",k)).collect::<Vec<_>>();
                println!("    {:8} => {:10} =[ {}", af_pin, af_pin_name, pin_names.join(" | "));
                // for mcu in mcus mcu..
            }
        }
    }
    
    Ok(())
}

/// Helpers (to be moved..)
fn convert_to_camel_case(s: &str) -> String {
    lazy_static! {
        static ref SEGMENT_RE: Regex = Regex::new(r#"(\d+)|([A-Z])([A-Z]+|[a-z]+)?|([a-z])([a-z]+)?"#).unwrap();
    }
    SEGMENT_RE
        .captures_iter(s)
        .map(|m| {
            let mut s;
            if let Some(r) = m.get(1) { // numbers
                s  = r.as_str().to_uppercase();
            } else if let Some(r) = m.get(2) { // big start
                s  = r.as_str().to_uppercase();
                if let Some(r) = m.get(3) {
                    s += &r.as_str().to_lowercase();
                }
            } else if let Some(r) = m.get(4) { // little start
                s  = r.as_str().to_uppercase();
                if let Some(r) = m.get(5) {
                    s += &r.as_str().to_lowercase();
                }
            } else { // impossible
                s = "".to_string();
            }
            s
        })
        .collect::<Vec<String>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpio_version_to_feature() {
        // Success
        assert_eq!(
            gpio_version_to_feature("STM32L152x8_gpio_v1_0").unwrap(),
            "io-STM32L152x8"
        );
        assert_eq!(
            gpio_version_to_feature("STM32F333_gpio_v1_0").unwrap(),
            "io-STM32F333"
        );

        // Error parsing, unsupported version
        assert!(gpio_version_to_feature("STM32F333_gpio_v1_1").is_err());

        // Error parsing, wrong pattern
        assert!(gpio_version_to_feature("STM32F333_qqio_v1_0").is_err());

        // Error parsing, too many underscores
        assert!(gpio_version_to_feature("STM32_STM32F333_gpio_v1_0").is_err());
    }
}
