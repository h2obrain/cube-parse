use std::{collections::HashMap, env, path::Path};

use alphanumeric_sort::compare_str;
use clap::{App, Arg};
use lazy_static::lazy_static;
use regex::Regex;

mod family;
mod internal_peripheral;
mod mcu;
mod utils;

use utils::ToPascalCase;

#[derive(Debug, PartialEq)]
enum GenerateTarget {
    QueryPinMappings,
    PinMappings,
    Features,
    PrintFamilies,
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
                .possible_values(&["query", "pin_mappings", "features", "print_families"])
                .required(false),
        )
        .arg(
            Arg::with_name("mcu_family")
                .help("The MCU family to extract, e.g. \"STM32L0\"")
                .takes_value(true)
                .required(false),
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
    let mcu_family = args.value_of("mcu_family");
    let generate = match args.value_of("generate").unwrap() {
        "query" => GenerateTarget::QueryPinMappings,
        "pin_mappings" => GenerateTarget::PinMappings,
        "features" => GenerateTarget::Features,
        "print_families" => GenerateTarget::PrintFamilies,
        _ => unreachable!(),
    };
    let af_stems = match args.values_of("af_stems") {
        Some(af_stems) => Some(af_stems.collect()),
        None => None,
    };

    // Load families
    let families = family::Families::load(&db_dir)
        .map_err(|e| format!("Could not load families XML: {}", e))?;

    // Print families
    if generate == GenerateTarget::PrintFamilies {
        //println!("Available mcu families:");
        for family in families.into_iter() {
            println!("  {}", family.name);
        }
        //println!();
        std::process::exit(0);
    }
    
    // Todo fix this..
    if !mcu_family.is_some() {
        eprintln!("mcu_family was not defined!");
        std::process::exit(0);
    }
    let mcu_family = mcu_family.unwrap();

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
        GenerateTarget::Features => generate_features(&mcu_gpio_map, &mcu_package_map, &mcu_family)?,
        GenerateTarget::PinMappings => {
            let af_tree = internal_peripheral::AfTree::build(&mcu_gpio_map, &db_dir)?;
            generate_pin_mappings(&af_tree, &af_stems)?;
        },
        GenerateTarget::QueryPinMappings => {
            let af_tree = internal_peripheral::AfTree::build(&mcu_gpio_map, &db_dir)?;
            display_af_tree(&af_tree, &af_stems, false)?;
        },
        GenerateTarget::PrintFamilies => (), // this point is never reached! 
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
        println!("gpio_version_feature: {:?} {:?}", gpio, mcu_list);
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

/// Display/query pin mappings from the AfTree.
fn display_af_tree(
    af_tree: &internal_peripheral::AfTree,
    af_stem_selection: &Option<Vec<&str>>,
    verbose: bool,
) -> Result<(), String> {
    for (stem,dev_map) in af_tree.iter(af_stem_selection)? {
        println!("{}", stem);
        for (dev,io_map) in dev_map {
            println!("  {}", dev);
            for ((af,io),(io_name,pin_map)) in io_map {
                if !verbose {
                    let pin_names = pin_map.keys().map(|k|format!("{:4}",k)).collect::<Vec<_>>();
                    println!("    {:4}: {:10} == {:8} =[ {}", af, io_name, io, pin_names.join(" | "));
                } else {
                    println!("    {:4}: {} ({})", af, io_name, io);
                    for (pin,(_pin_letter,_pin_number,gpio_map)) in pin_map {
                        println!("      {}", pin);
                        for (gpio_mcu,versions) in gpio_map {
                            println!("        gpio-group: {}", gpio_mcu);
                            #[allow(clippy::never_loop)]
                            for (version,mcus) in versions {
                                println!("        gpio-version: {}", version);
                                for mcu in (*mcus).iter() {
                                    println!("          {}", mcu);
                                }
                                // fixme
                                if versions.len() > 1 {
                                    eprintln!("Multiple gpio-versions not supported! {:?}", versions.keys());
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Generate the pin mappings for the AfTree.
fn generate_pin_mappings(
    af_tree: &internal_peripheral::AfTree,
    af_stem_selection: &Option<Vec<&str>>,
) -> Result<(), String> {
    // TODO maybe add build_tree(do-not-distinguish-af) mode for uses:String
    
    for (stem,dev_map) in af_tree.iter(af_stem_selection)? {
        let mut uses = String::new();
        let mut traits = String::new();
        let mut ios:Vec<&str> = Vec::new();
        let mut pins_where = String::new();
        let mut implementations = String::new();
        
        let stem = stem.as_str().to_pascalcase();
        traits.push_str(format!("pub trait Pins<{}> {{}}\n", stem).as_str());
        
        for (dev,io_map) in dev_map {
            for ((af,io),(io_name,pin_map)) in io_map {
                traits.push_str(format!("pub trait {}<{}> {{}}\n", io_name,stem).as_str());
                ios.push(io.as_str());
                pins_where.push_str(format!("    {}: {}<{}>,\n", io, io_name, stem).as_str());
                for (pin,(pin_letter,_pin_number,gpio_map)) in pin_map {
                    let mut feature_selection = "#[cfg(any(\n".to_string();
//                    for (gpio_mcu,versions) in gpio_map {
                    for versions in gpio_map.values() {
                        #[allow(clippy::never_loop)]
//                        for (version,mcus) in versions {
                        for mcus in versions.values() {
                            for mcu in (*mcus).iter() {
                                feature_selection.push_str(format!("    feature = \"{}\",\n", mcu).as_str());
                            }
                            // fixme
                            if versions.len() > 1 {
                                eprintln!("Multiple gpio-versions not supported! {:?}", versions.keys());
                            }
                            break;
                        }
                    }
                    feature_selection.push_str("))]");
                    
                    // uses
//                    uses.push_str(format!("
//{}
//use crate::gpio::gpio{}::{};
//",
//                        feature_selection,
//                        pin_letter.to_lowercase(),
//                        pin
//                    ).as_str());
//
//                    // implementations
//                    implementations.push_str(format!("
//{}
//impl {}<{}> for {}<{}> {{}}
//",
//                        feature_selection,
//                        io_name, dev, pin, af
//                    ).as_str());
//                }
//            }
//        }
//    }

                    // implementations
                    implementations.push_str(format!("
{}
impl {}<{}> for gpio::gpio{}::{}<gpio::{}> {{}}
",
                        feature_selection,
                        io_name, dev, pin_letter.to_lowercase(), pin, af
                    ).as_str());
                }
            }
        }
        
        let ios = ios.join(", ");
        let pins = format!("
impl<{}, {}> Pins<{}> for ({})
where
{}{{}}",
            stem, ios, stem, ios, pins_where
        );
        
        uses.push_str("use crate::gpio;\n\n");
        println!("\n############################################\n{}", uses);
        println!("\n############################################\n{}", traits);
        println!("\n############################################\n{}", pins);
        println!("\n############################################\n{}", implementations);
    }
    
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
