use std::{collections::HashMap, env, path::Path};

use alphanumeric_sort::compare_str;
use clap::{App, Arg};
use lazy_static::lazy_static;
use regex::Regex;

mod family;
mod internal_peripheral;
mod mcu;
mod utils;

use internal_peripheral::{AfSignal, SignalType};

#[derive(Debug, PartialEq)]
enum GenerateTarget {
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
                .possible_values(&["pin_mappings", "features"])
                .required(true),
        )
        .arg(
            Arg::with_name("mcu_family")
                .help("The MCU family to extract, e.g. \"STM32L0\"")
                .takes_value(true)
                .required(true),
        )
        .get_matches();

    // Process args
    let db_dir = Path::new(args.value_of("db_dir").unwrap());
    let mcu_family = args.value_of("mcu_family").unwrap();
    let generate = match args.value_of("generate").unwrap() {
        "pin_mappings" => GenerateTarget::PinMappings,
        "features" => GenerateTarget::Features,
        _ => unreachable!(),
    };

    // Load families
    let families = family::Families::load(&db_dir)
        .map_err(|e| format!("Could not load families XML: {}", e))?;

    // Find target family
    let family = (&families)
        .into_iter()
        .find(|v| v.name == mcu_family)
        .ok_or_else(|| format!("Could not find family {}", mcu_family))?;

    // Build MCU map
    //
    // The keys of this map are GPIO peripheral version strings (e.g.
    // "STM32L051_gpio_v1_0"), while the value is a Vec of MCU ref names.
    let mut mcu_gpio_map: HashMap<String, Vec<String>> = HashMap::new();
    for sf in family {
        for mcu in sf {
            let mcu_dat = mcu::Mcu::load(&db_dir, &mcu.name)
                .map_err(|e| format!("Could not load MCU data: {}", e))?;
            let gpio_version = mcu_dat.get_ip("GPIO").unwrap().get_version().to_string();
            mcu_gpio_map
                .entry(gpio_version)
                .or_insert(vec![])
                .push(mcu.ref_name.clone());
        }
    }

    match generate {
        GenerateTarget::Features => generate_features(&mcu_gpio_map)?,
        GenerateTarget::PinMappings => generate_pin_mappings(&mcu_gpio_map, &db_dir)?,
    };

    Ok(())
}

/// Print the IO features, followed by MCU features that act purely as aliases
/// for the IO features.
///
/// Both lists are sorted alphanumerically.
fn generate_features(mcu_gpio_map: &HashMap<String, Vec<String>>) -> Result<(), String> {
    let mut main_features = mcu_gpio_map
        .keys()
        .map(|gpio| gpio_version_to_feature(gpio))
        .collect::<Result<Vec<String>, String>>()?;
    main_features.sort();

    let mut mcu_aliases = vec![];
    for (gpio, mcu_list) in mcu_gpio_map {
        let gpio_version_feature = gpio_version_to_feature(gpio).unwrap();
        for mcu in mcu_list {
            let mcu_feature = format!("mcu-{}", mcu);
            mcu_aliases.push(format!("{} = [\"{}\"]", mcu_feature, gpio_version_feature));
        }
    }
    mcu_aliases.sort();

    println!("// Features based on the GPIO peripheral version.");
    println!("// This determines the pin function mapping of the MCU.");
    for feature in main_features {
        println!("{} = []", feature);
    }
    println!("\n// Per-MCU aliases for the GPIO peripheral version.");
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
        let signal_mapping = [(SignalType::Rx, "RxPin"), (SignalType::Tx, "TxPin")]
            .iter()
            .cloned()
            .collect::<HashMap<_, _>>();
        render_pin_modes(&gpio_data, &signal_mapping)?;
        println!("\n");
    }
    Ok(())
}

fn render_pin_mode_item(
    name: &str,
    signal: &AfSignal,
    signal_mapping: &HashMap<SignalType, &'static str>,
    standalone: bool,
) {
    let signal_trait = signal_mapping.get(&signal.signal_type).unwrap();
    if standalone {
        println!(
            "    {} => {{AF{}: {}<{}>}},",
            name, signal.af, signal_trait, signal.peripheral
        );
    } else {
        println!(
            "        AF{}: {}<{}>,",
            signal.af, signal_trait, signal.peripheral
        );
    }
}

fn render_pin_modes(
    ip: &internal_peripheral::IpGPIO,
    signal_mapping: &HashMap<SignalType, &'static str>,
) -> Result<(), String> {
    let mut pin_map: HashMap<String, Vec<AfSignal>> = HashMap::new();

    for p in &ip.gpio_pin {
        if let Some(name) = p.get_name() {
            pin_map.insert(name, p.get_af_modes()?);
        }
    }

    let mut pin_map = pin_map
        .into_iter()
        .map(|(name, mut signals)| {
            signals.sort_by_key(|x: &AfSignal| x.af);
            (
                name,
                signals
                    .iter()
                    .filter(|s: &&AfSignal| signal_mapping.contains_key(&s.signal_type))
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();

    pin_map.sort_by(|a, b| compare_str(&a.0, &b.0));

    println!("pins! {{");
    for (name, signals) in pin_map {
        if signals.is_empty() {
            continue;
        } else if signals.len() == 1 {
            render_pin_mode_item(&name, &signals[0], signal_mapping, true);
        } else {
            println!("    {} => {{", name);
            for signal in signals {
                render_pin_mode_item(&name, &signal, signal_mapping, false);
            }
            println!("    }},");
        }
    }
    println!("}}");

    Ok(())
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
