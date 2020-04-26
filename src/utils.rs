use std::{error::Error, fs::File, io::BufReader, path::Path, fmt, cmp::Ordering, hash::{Hash,Hasher}};
use serde::Deserialize;
use lazy_static::lazy_static;
use regex::Regex;
use alphanumeric_sort::compare_str;

pub fn load_file<'a, P: AsRef<Path>, Q: AsRef<Path>, R: Deserialize<'a>>(
    db_dir: P,
    file_path: Q,
) -> Result<R, Box<dyn Error>> {
    let db_dir = db_dir.as_ref();
    let mut fin = BufReader::new(File::open(&db_dir.join(file_path.as_ref()))?);

    Ok(serde_xml_rs::deserialize(&mut fin)?)
}


/// Camel Case
pub trait ToPascalCase {
    fn to_pascalcase(&self) -> String;
}
impl ToPascalCase for str {
    fn to_pascalcase(&self) -> String {
        lazy_static! {
            static ref SEGMENT_RE: Regex = Regex::new(r#"(\d+)|([A-Z])([A-Z]+|[a-z]+)?|([a-z])([a-z]+)?"#).unwrap();
        }
        SEGMENT_RE
            .captures_iter(self)
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
}

/// SortedString
#[derive(Clone, /*Hash,*//* Eq,*/ /*PartialEq,*/ /*PartialOrd*//*, Debug*/)]
pub struct SortedString(pub String);
impl Ord for SortedString {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_str(self.0.as_str(), other.0.as_str())
    }
}
impl PartialOrd for SortedString {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Eq for SortedString {
}
impl PartialEq for SortedString {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}
impl PartialEq<&SortedString> for SortedString {
    fn eq(&self, other: &&Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}
//impl PartialEq<SortedString> for &SortedString {
//    fn eq(&&self, other: &Self) -> bool {
//        self.0.as_str() == other.0.as_str()
//    }
//}
impl Hash for SortedString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}
impl fmt::Debug for SortedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (&self.0).fmt(f)
    }
}
impl fmt::Display for SortedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (&self.0).fmt(f)
    }
}
impl SortedString {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
pub trait ToSortedString {
    fn to_sorted_string(&self) -> SortedString;
}
impl ToSortedString for &str {
    fn to_sorted_string(&self) -> SortedString {
        SortedString((*self).to_string())
    }
}
impl ToSortedString for &String {
    fn to_sorted_string(&self) -> SortedString {
        SortedString((*self).to_string())
    }
}
impl ToSortedString for String {
    fn to_sorted_string(&self) -> SortedString {
        SortedString(self.to_string())
    }
}
