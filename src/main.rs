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

use std::collections::{BTreeSet,BTreeMap};
use utils::{SortedString,ToSortedString,BreakLine};


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
    if mcu_family.is_none() {
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
                .or_insert_with(Vec::new)
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
            let af_tree = internal_peripheral::AfTree::build(mcu_family, &mcu_gpio_map, &db_dir, true)?;
            generate_pin_mappings(&af_tree, &af_stems, true)?;
        },
        GenerateTarget::QueryPinMappings => {
            let af_tree = internal_peripheral::AfTree::build(mcu_family, &mcu_gpio_map, &db_dir, true)?;
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
                        dependencies.push((*feature).to_string());
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


/// Example loop for AfTree
//fn generate_pin_mappings(
//    af_tree: &internal_peripheral::AfTree,
//    af_stem_selection: &Option<Vec<&str>>,
//) -> Result<(), String> {
//    for (stem,dev_map) in af_tree.iter(af_stem_selection)? {
//        for (dev,io_map) in dev_map {
//            for ((af,io),(io_name,pin_map)) in io_map {
//                for ((port_name,pin_nr),(_original_pin_names,gpio_map)) in pin_map {
//                    for (gpio_mcu,versions) in gpio_map {
//                        #[allow(clippy::never_loop)]
//                        for (version,mcus) in versions {
//                            for mcu in (*mcus).iter() {
//                            }
//                            // fixme
//                            if versions.len() > 1 {
//                                eprintln!("Multiple gpio-versions not supported! {:?}", versions.keys());
//                            }
//                            break;
//                        }
//                    }
//                }
//            }
//        }
//    }
//    Ok(())
//}


/// Display/query pin mappings from the AfTree.
fn display_af_tree(
    af_tree: &internal_peripheral::AfTree,
    af_stem_selection: &Option<Vec<&str>>,
    verbose: bool,
) -> Result<(), String> {
    for (stem, dev_map) in af_tree.iter(af_stem_selection)? {
        println!("{}", stem);
        for (dev,io_map) in dev_map {
            println!("  {}", dev);
            for ((af, io), (io_name, pin_map)) in io_map {
                if !verbose {
                    let pin_names = pin_map.keys().map(|(p,nr)|format!("{}{:<2}",p,nr)).collect::<Vec<_>>();
                    println!("    {:4}: {:10} == {:8} =[ {}", af, io_name, io, pin_names.join(" | "));
                } else {
                    println!("    {:4}: {} ({})", af, io_name, io);
                    for ((port_name,pin_nr),(_original_pin_names,gpio_map)) in pin_map {
                        println!("      {}{}", port_name,pin_nr);
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
    combine_mcu_lists: bool,
) -> Result<(), String> {
    // running 2nd pass on af-analysis (1st pass being building the af-tree)
    // collecting data without any efficiency in mind :)
    // probably group for each stem or so..
    // maybe don't extend gpio-version to mcu-list in af-tree

    // IO traits
//    let mut io_traits: BTreeMap<SortedString, BTreeSet<(SortedString,&str)>> = BTreeMap::new();
//    for (stem,dev_map) in af_tree.iter(af_stem_selection)? {
//        for io_map in dev_map.values() {
//            for ((_af,io), (io_name,_pin_map)) in io_map {
//                io_traits.entry(stem.as_str().to_pascalcase().to_sorted_string()).or_insert_with(BTreeSet::new)
//                    .insert((io_name.to_sorted_string(),io.as_str()));
//            }
//        }
//    }

    // Devices used per mcu
    let mut devs: BTreeMap<BTreeSet<&SortedString>, BTreeSet<SortedString>> = BTreeMap::new();
    // AF used per mcu
    let mut gpio_afs: BTreeMap<BTreeSet<&SortedString>, BTreeSet<SortedString>> = BTreeMap::new();
    // Gpio pins used per mcu
    #[allow(clippy::type_complexity)]
    let mut gpios: BTreeMap<BTreeSet<&SortedString>, BTreeMap<SortedString, BTreeSet<(String,u32)>>> = BTreeMap::new();

    // IO traits per mcu
    #[allow(clippy::type_complexity)]
    let mut io_traits: BTreeMap<BTreeSet<&SortedString>, BTreeSet<(SortedString, &str)>> = BTreeMap::new();
    #[allow(clippy::type_complexity)]
    let mut io_traits_by_peripheral: BTreeMap<BTreeSet<&SortedString>, BTreeMap<SortedString, BTreeSet<(SortedString,&str)>>> = BTreeMap::new();

    // Pin trait implementations per mcu
    #[allow(clippy::type_complexity)]
    let mut mct: BTreeMap<BTreeSet<&SortedString>, BTreeSet<(SortedString,u32,SortedString,SortedString,SortedString)>> = BTreeMap::new();

    if combine_mcu_lists {
        // combine mcus per pin-def
        #[allow(clippy::type_complexity)]
        let mut devs_collect: BTreeMap<SortedString, BTreeSet<&SortedString>> = BTreeMap::new();
        #[allow(clippy::type_complexity)]
        let mut gpio_afs_collect: BTreeMap<SortedString, BTreeSet<&SortedString>> = BTreeMap::new();
        #[allow(clippy::type_complexity)]
        let mut gpios_collect: BTreeMap<(String,u32), BTreeSet<&SortedString>> = BTreeMap::new();
        #[allow(clippy::type_complexity)]
        let mut io_traits_collect: BTreeMap<(SortedString,&str), BTreeSet<&SortedString>> = BTreeMap::new();
        #[allow(clippy::type_complexity)]
        let mut io_traits_collect_by_peripheral: BTreeMap<(SortedString,SortedString,&str), BTreeSet<&SortedString>> = BTreeMap::new();
        for (stem,dev_map) in af_tree.iter(af_stem_selection)? {
            for (dev,io_map) in dev_map {
                let mut grouped_mcus_dev: BTreeSet<&SortedString> = BTreeSet::new();
                for ((af,io),(io_name,pin_map)) in io_map {
                    let mut grouped_mcus_af: BTreeSet<&SortedString> = BTreeSet::new();
                    for ((port_name,pin_nr),(_original_pin_names,gpio_map)) in pin_map {
                        let mut grouped_mcus: BTreeSet<&SortedString> = BTreeSet::new();
                        for versions in gpio_map.values() {
                            #[allow(clippy::never_loop)]
                            for mcus in versions.values() {
                                grouped_mcus.extend((*mcus).iter());
                                if versions.len() > 1 {
                                    eprintln!("Multiple gpio-versions not supported! {:?}", versions.keys());
                                }
                                break;
                            }
                        }
                        mct.entry(grouped_mcus.to_owned()).or_insert_with(BTreeSet::new).insert((
                                // note, the order here is important (see below: (p,n, af, ion, dev))
                                port_name.to_sorted_string(),*pin_nr,
                                af.to_owned(),
                                io_name.to_sorted_string(),
                                dev.to_owned()
                            ));
                        gpios_collect.entry((port_name.to_owned(), *pin_nr)).or_insert_with(BTreeSet::new)
                            .extend(grouped_mcus.iter());
                        grouped_mcus_af.extend(grouped_mcus.iter());
                    }
                    // Collect the io_traits by independent of the peripheral (stem)
                    io_traits_collect
                        .entry((io_name.to_sorted_string(),io.as_str())).or_insert_with(BTreeSet::new)
                        .extend(grouped_mcus_af.iter());
                    io_traits_collect_by_peripheral
                        .entry((stem.to_owned(),io_name.to_sorted_string(),io.as_str())).or_insert_with(BTreeSet::new)
                        .extend(grouped_mcus_af.iter());
                    gpio_afs_collect.entry(af.to_owned()).or_insert_with(BTreeSet::new).extend(grouped_mcus_af.iter());
                    grouped_mcus_dev.extend(grouped_mcus_af.iter());
                }
                devs_collect.entry(dev.to_owned()).or_insert_with(BTreeSet::new).extend(grouped_mcus_dev.iter());
            }
        }
        for ((io_name,io), mcus) in io_traits_collect {
            io_traits
                .entry(mcus.to_owned()).or_insert_with(BTreeSet::new)
                .insert((io_name,io));
        }
        for ((stem,io_name,io), mcus) in io_traits_collect_by_peripheral {
            io_traits_by_peripheral
                .entry(mcus.to_owned()).or_insert_with(BTreeMap::new)
                .entry(stem.as_str().to_pascalcase().to_sorted_string()).or_insert_with(BTreeSet::new)
                .insert((io_name,io));
        }
        for (dev, mcus) in devs_collect {
            devs.entry(mcus.to_owned()).or_insert_with(BTreeSet::new).insert(dev);
        }
        for (gpio_af, mcus) in gpio_afs_collect {
            gpio_afs.entry(mcus.to_owned()).or_insert_with(BTreeSet::new).insert(gpio_af);
        }
        for (gpio, mcus) in gpios_collect {
            gpios.entry(mcus.to_owned()).or_insert_with(BTreeMap::new)
                .entry(format!("gpio{}", gpio.0.as_str()[1..].to_lowercase()).to_sorted_string()).or_insert_with(BTreeSet::new)
                .insert(gpio);
        }
    } else {
        // leave the original mcu groups
        for (stem,dev_map) in af_tree.iter(af_stem_selection)? {
            for (dev,io_map) in dev_map {
                for ((af,io),(io_name,pin_map)) in io_map {
                    for ((port_name,pin_nr),(_original_pin_names,gpio_map)) in pin_map {
                        for versions in gpio_map.values() {
                            #[allow(clippy::never_loop)]
                            for mcus in versions.values() {
                                mct.entry(mcus.iter().collect()).or_insert_with(BTreeSet::new).insert((
                                    // note, the order here is important (see below: (p,n, af, ion, dev))
                                    port_name.to_sorted_string(),*pin_nr,
                                    af.to_owned(),
                                    io_name.to_sorted_string(),
                                    dev.to_owned()
                                ));
                                io_traits.entry(mcus.iter().collect()).or_insert_with(BTreeSet::new)
                                    .insert((io_name.to_sorted_string(),io.as_str()));
                                io_traits_by_peripheral.entry(mcus.iter().collect()).or_insert_with(BTreeMap::new)
                                    .entry(stem.as_str().to_pascalcase().to_sorted_string()).or_insert_with(BTreeSet::new)
                                    .insert((io_name.to_sorted_string(),io.as_str()));
                                gpios.entry(mcus.iter().collect()).or_insert_with(BTreeMap::new)
                                    .entry(format!("gpio{}", port_name.as_str()[1..].to_lowercase()).to_sorted_string())
                                    .or_insert_with(BTreeSet::new)
                                    .insert((port_name.to_owned(), *pin_nr));
                                gpio_afs.entry(mcus.iter().collect()).or_insert_with(BTreeSet::new).insert(af.to_owned());
                                devs.entry(mcus.iter().collect()).or_insert_with(BTreeSet::new).insert(dev.to_owned());

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
    
    // IO traits per mcu (not really needed..)
    #[allow(clippy::type_complexity)]
    let mut io_traits_grouped: BTreeMap<BTreeSet<&SortedString>, BTreeMap<&SortedString, BTreeSet<(&SortedString,&str)>>> = BTreeMap::new();
    {
        #[allow(clippy::type_complexity)]
        let mut iot_ex: BTreeMap<&SortedString, BTreeMap<&SortedString, BTreeSet<(&SortedString,&str)>>> = BTreeMap::new();
        for (mcus, iot) in &io_traits_by_peripheral {
            for mcu in mcus {
                for (gpio, ios) in iot {
                    for (io_name, io) in ios {
                        iot_ex
                            .entry(mcu).or_insert_with(BTreeMap::new)
                            .entry(gpio).or_insert_with(BTreeSet::new)
                            .insert((io_name,io));
                    }
                }
            }
        }
        while !iot_ex.is_empty() {
            let mut ii = iot_ex.iter();
            let mi = ii.next().unwrap();
            let mut mcus: BTreeSet<&SortedString>;
            mcus = ii.filter_map(|(mcu, iot)| if mi.1==iot { Some(*mcu) } else { None }).collect();
            mcus.insert(mi.0);
            let ios = mi.1.to_owned();
            if io_traits_grouped.contains_key(&mcus) {
                eprintln!("HOW?? Duplicated mcu group? ({:?})", mcus);
            }
            for mcu in &mcus {
                iot_ex.remove(mcu);
            }
            io_traits_grouped.insert(mcus, ios);
        }        
    }
        
    
    // formatting collected data
    
    // uses
    let mut uses = String::new();
    uses.push_str("
use crate::gpio::Alternate;

macro_rules! dev_uses {
    ($($DEV:ident),+) => {
        $(
            use crate::stm32::$DEV;
        )+
    }
}
macro_rules! gpio_af_uses {
    ($($AF:ident),+) => {
        use crate::gpio::{$($AF),+};
    }
}
macro_rules! gpio_uses {
    ($($GPIO:ident => {
        $($PINS:ident),+
    }),+) => {
        $(
            use crate::gpio::$GPIO::{$($PINS),+};
        )+
    }
}
");
    
    // devices uses
    for (mcus, devs) in devs {
        uses.push_str(format!(
"
#[cfg(any(
{}
))]
dev_uses! {{
    {}
}}
",          mcus.iter().map(|mcu|
                format!("    feature = \"{}\"", mcu)
            ).collect::<Vec<_>>().join(",\n"),
            devs.iter().map(|dev|dev.to_string()).collect::<Vec<_>>().join(", ")
        ).as_str());
    }
    // alternate function (AFx) uses
    for (mcus, gpio_afs) in gpio_afs {
        uses.push_str(format!(
"
#[cfg(any(
{}
))]
gpio_af_uses! {{
    {}
}}
",          mcus.iter().map(|mcu|
                format!("    feature = \"{}\"", mcu)
            ).collect::<Vec<_>>().join(",\n"),
            gpio_afs.iter().map(|af|af.to_string()).collect::<Vec<_>>().join(", ")
        ).as_str());
    }
        
    for (mcus, gpios) in gpios {
        uses.push_str(format!(
"
#[cfg(any(
{}
))]
gpio_uses! {{
{}
}}
",          mcus.iter().map(|mcu|
                format!("    feature = \"{}\"", mcu)
            ).collect::<Vec<_>>().join(",\n"),
            gpios.iter().map(|(gpio,pins)|
                format!(
                    "    {} => {{{}}}",
                    gpio,
                    pins.iter().map(|(p,n)| format!("{}{}",p,n))
                        .collect::<Vec<_>>().join(", ")
                        .break_line(10,50,"\n        ","\n        ","\n    ")
                )
            ).collect::<Vec<_>>().join(",\n")
        ).as_str());
    }
    
    // Define traits
    let mut traits = String::new();
    traits.push_str("
macro_rules! io_traits {
    ($($STEM:ident => {
        $($IO:ident),+
    }),+) => {
        $(
            $(
                pub trait $IO<$STEM> {}
            )+
        )+
    }
}");
    for (mcus, io_traits) in &io_traits {
        traits.push_str(format!(
"
#[cfg(any(
{}
))]
io_traits! {{
    Dev => {{{}}}
}}
",          mcus.iter().map(|mcu|
                format!("    feature = \"{}\"", mcu)
            ).collect::<Vec<_>>().join(",\n"),
            io_traits
                .iter().map(|(ion,_io)|ion.to_string())
                .collect::<Vec<_>>().join(", ")
                .break_line(10,50,"\n        ","\n        ","\n    ")
//            io_traits.iter().map(|(stem,ions)|
//                format!("   {} => {{{}}}",
//                    stem,
//                format!("   Dev => {{{}}}",
//                    ions.iter().map(|(ion,_io)|ion.to_string())
//                        .collect::<Vec<_>>().join(", ")
//                        .break_line(10,50,"\n        ","\n        ","\n    ")
//                )).collect::<Vec<_>>().join(",\n")
            ).as_str());
    }
    
    // Implement traits for the pins
    let mut implementations = String::new();
    implementations.push_str("
macro_rules! pins {
    ($($PIN:ident => {
        $($AF:ty: $TRAIT:ty),+
    }),+) => {
        $(
            $(
                impl $TRAIT for $PIN<Alternate<$AF>> {}
            )+
        )+
    }
}

");
    for (mcus, pins) in mct {
        implementations.push_str(format!(
"
#[cfg(any(
{}
))]
pins! {{
{}
}}
",          mcus.iter().map(|mcu|
                format!("    feature = \"{}\"", mcu)
            ).collect::<Vec<_>>().join(",\n"),
            pins.iter().map(|(p,n, af, ion, dev)|
                format!(
                    "    {}{:<2} => {{{:4}: {}<{}>}}",
                    p,n,
                    af, ion, dev
                )
            ).collect::<Vec<_>>().join(",\n"),
        ).as_str());
    }
    
    // Define Pins<stem>
    // NOTE: this should always be hand-edited!
    let mut pins = String::new();
    for (mcus, io_traits) in io_traits_grouped {
        pins.push_str(format!(
"
#[cfg(any(
{}
))] mod pins {{
    use crate::pin_defs::*;
{}
}}
",          mcus.iter().map(|mcu|
                format!("    feature = \"{}\"", mcu)
            ).collect::<Vec<_>>().join(",\n"),
            io_traits.iter().map(|(stem,ions)| {
                let all_io = ions.iter().map(|(_ion,io)|(*io).to_string())
                                 .collect::<Vec<_>>().join(", ")
                                 .break_line(10,50,"\n        ","\n        ","\n    ");
                format!("    /// {}
    pub trait Pins<{}> {{}}
    impl<{}, {}> Pins<{}> for ({})
    where
{}
    {{}}
",                  stem,
                    stem,
                    stem, all_io, stem, all_io,
                    ions.iter().map(|(ion,io)|format!(
                        "        {}: {}<{}>",
                        io,ion,stem
                    )).collect::<Vec<_>>().join(",\n")
                )
            }).collect::<Vec<_>>().join("\n")
        ).as_str());
    }
    
    // Write results to stdout
    println!("
// Uses
{}


// Traits
{}


// Implementations
{}


//////////////////////////////////////////////////////////////////////////////
//////////////////////////////////////////////////////////////////////////////
//////////////////////////////////////////////////////////////////////////////
//////////////////////////////////////////////////////////////////////////////
//////////////////////////////////////////////////////////////////////////////
/////////////////////                               //////////////////////////
////////////////////// Pins (remove/edit by hand!) ///////////////////////////
////////////////////// Those definitions belong in ///////////////////////////
////////////////////// the device implementation   ///////////////////////////
////////////////////// files and not into the      ///////////////////////////
////////////////////// pin_defs.h                  ///////////////////////////
/////////////////////                               //////////////////////////
//////////////////////////////////////////////////////////////////////////////
//////////////////////////////////////////////////////////////////////////////
//////////////////////////////////////////////////////////////////////////////
{}
",      uses,
        traits,
        implementations,
        pins
    );
    
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
