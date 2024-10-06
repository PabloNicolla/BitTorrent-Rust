use anyhow::Context;
use clap::{Parser, Subcommand};
use hashes::Hashes;
use serde::Deserialize;
use serde_bencode;
use serde_json;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand, Debug)]
enum Command {
    Decode { value: String },
    Info { torrent: PathBuf },
}
/// A Metainfo file (also known as .torrent files).
#[derive(Debug, Clone, Deserialize)]
struct Torrent {
    /// The URL of the tracker.
    announce: String,
    info: Info,
}
#[derive(Debug, Clone, Deserialize)]
struct Info {
    /// The suggested name to save the file (or directory) as. It is purely advisory.
    ///
    /// In the single file case, the name key is the name of a file, in the muliple file case, it's
    /// the name of a directory.
    name: String,
    /// The number of bytes in each piece the file is split into.
    ///
    /// For the purposes of transfer, files are split into fixed-size pieces which are all the same
    /// length except for possibly the last one which may be truncated. piece length is almost
    /// always a power of two, most commonly 2^18 = 256K (BitTorrent prior to version 3.2 uses 2
    /// 20 = 1 M as default).
    #[serde(rename = "piece length")]
    plength: usize,
    /// Each entry of `pieces` is the SHA1 hash of the piece at the corresponding index.
    pieces: Hashes,
    #[serde(flatten)]
    keys: Keys,
}
/// There is a key `length` or a key `files`, but not both or neither.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Keys {
    /// If `length` is present then the download represents a single file.
    SingleFile {
        /// The length of the file in bytes.
        length: usize,
    },
    /// Otherwise it represents a set of files which go in a directory structure.
    ///
    /// For the purposes of the other keys in `Info`, the multi-file case is treated as only having
    /// a single file by concatenating the files in the order they appear in the files list.
    MultiFile { files: Vec<File> },
}
#[derive(Debug, Clone, Deserialize)]
struct File {
    /// The length of the file, in bytes.
    length: usize,
    /// Subdirectory names for this file, the last of which is the actual file name
    /// (a zero length list is an error case).
    path: Vec<String>,
}
// Usage: your_bittorrent.sh decode "<encoded_value>"
fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match args.command {
        Command::Decode { value } => {
            let decoded_value = decode_bencoded_value(&value);
            println!("{}", decoded_value.to_string());
        }
        Command::Info { torrent } => {
            let dot_torrent = std::fs::read(torrent).context("read torrent file")?;
            let t: Torrent =
                serde_bencode::from_bytes(&dot_torrent).context("parse torrent file")?;
            eprintln!("{t:?}");
            println!("Tracker URL: {}", t.announce);
            if let Keys::SingleFile { length } = t.info.keys {
                println!("Length: {length}");
            } else {
                todo!();
            }
        }
    }
    Ok(())
}
mod hashes {
    use serde::de::{self, Deserialize, Deserializer, Visitor};
    use std::fmt;
    #[derive(Debug, Clone)]
    pub struct Hashes(pub Vec<[u8; 20]>);
    struct HashesVisitor;
    impl<'de> Visitor<'de> for HashesVisitor {
        type Value = Hashes;
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a byte string whose length is a multiple of 20")
        }
        fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if v.len() % 20 != 0 {
                return Err(E::custom(format!("length is {}", v.len())));
            }
            // TODO: use array_chunks when stable
            Ok(Hashes(
                v.chunks_exact(20)
                    .map(|slice_20| slice_20.try_into().expect("guaranteed to be length 20"))
                    .collect(),
            ))
        }
    }
    impl<'de> Deserialize<'de> for Hashes {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_bytes(HashesVisitor)
        }
    }
}

mod bencode {
    #[derive(Debug)]
    pub enum BencodeError {
        ParseError(String),
        Other(String),
    }

    impl BencodeError {
        fn parse_error(msg: &str) -> Self {
            BencodeError::ParseError(msg.to_string())
        }
        fn other_error(msg: &str) -> Self {
            BencodeError::Other(msg.to_string())
        }
    }

    pub enum BencodeType {
        BtString(String),
        BtNumber(i64),
        BtLists(Vec<serde_json::Value>),
        BtDictionary(serde_json::Value),
    }

    pub struct BencodeDecoder<'a> {
        encoded_value: &'a str,
        start_pos: usize,
    }

    impl<'a> BencodeDecoder<'a> {
        pub fn new(encoded_value: &'a str) -> BencodeDecoder<'a> {
            BencodeDecoder {
                encoded_value,
                start_pos: 0,
            }
        }

        pub fn decode(&mut self) -> Result<serde_json::Value, BencodeError> {
            match self.discover_bencoding_type() {
                Ok(BencodeType::BtString(decoded_str)) => {
                    Ok(serde_json::Value::String(decoded_str))
                }
                Ok(BencodeType::BtNumber(decoded_number)) => Ok(decoded_number.into()),
                Ok(BencodeType::BtLists(decoded_list)) => Ok(decoded_list.into()),
                Ok(BencodeType::BtDictionary(decoded_dic)) => Ok(decoded_dic),
                Err(e) => {
                    eprintln!("{e:?}");
                    Err(e)
                }
            }
        }

        fn discover_bencoding_type(&mut self) -> Result<BencodeType, BencodeError> {
            let cur_range = &self.encoded_value[self.start_pos..];
            let cur_char = cur_range.chars().next().ok_or(BencodeError::parse_error(
                "Invalid encoding format, no character to parse",
            ))?;

            if cur_char.is_digit(10) {
                return self.parse_bt_string();
            } else if 'i' == cur_char {
                return self.parse_bt_integer();
            } else if 'l' == cur_char {
                return self.parse_bt_list();
            } else if 'd' == cur_char {
                return self.parse_bt_dic();
            } else {
                Err(BencodeError::Other(format!(
                    "Unhandled encoded value: {}",
                    self.encoded_value
                )))
            }
        }

        fn parse_bt_string(&mut self) -> Result<BencodeType, BencodeError> {
            let cur_range = &self.encoded_value[self.start_pos..];
            let colon_index = cur_range.find(':').ok_or(BencodeError::parse_error(
                "Invalid encoding format for string, colon separator not found",
            ))?;
            let number_string = &cur_range[..colon_index];
            let number = number_string.parse::<i64>().map_err(|_| {
                BencodeError::parse_error(
                    "Invalid encoding format for string, invalid encoded length",
                )
            })?;
            let end = colon_index + 1 + number as usize;
            let string = &cur_range[colon_index + 1..end];
            self.start_pos += end;
            return Ok(BencodeType::BtString(string.to_string()));
        }

        fn parse_bt_integer(&mut self) -> Result<BencodeType, BencodeError> {
            let cur_range = &self.encoded_value[self.start_pos..];
            let e_index = cur_range.find('e').ok_or(BencodeError::parse_error(
                "Invalid encoding format for number, `e` delimiter not found",
            ))?;
            if e_index == 1 {
                return Err(BencodeError::parse_error(
                    "Invalid encoding format for number, trying to parse `ie`",
                ));
            }
            let number_string = &cur_range[1..e_index];
            let number = number_string.parse::<i64>().map_err(|_| {
                BencodeError::parse_error(
                    "Invalid encoding format for number, invalid encoded number",
                )
            })?;
            self.start_pos += e_index + 1;
            Ok(BencodeType::BtNumber(number))
        }

        fn parse_bt_list(&mut self) -> Result<BencodeType, BencodeError> {
            let mut list: Vec<serde_json::Value> = Vec::new();
            self.start_pos += 1;

            loop {
                let cur_range = &self.encoded_value[self.start_pos..];
                let first_char = cur_range.chars().next().ok_or_else(|| {
                    BencodeError::parse_error("Invalid encoding format, incomplete list encoding")
                })?;
                if first_char == 'e' {
                    self.start_pos += 1;
                    return Ok(BencodeType::BtLists(list));
                }
                list.push(self.decode()?)
            }
        }

        fn parse_bt_dic(&mut self) -> Result<BencodeType, BencodeError> {
            let mut dict = serde_json::Map::new();
            self.start_pos += 1;

            loop {
                let cur_range = &self.encoded_value[self.start_pos..];
                let first_char = cur_range.chars().next().ok_or_else(|| {
                    BencodeError::parse_error("Invalid encoding format, incomplete dict encoding")
                })?;
                if first_char == 'e' {
                    self.start_pos += 1;
                    return Ok(BencodeType::BtDictionary(dict.into()));
                }
                let next_decoded_val = self.discover_bencoding_type()?;
                if let BencodeType::BtString(key) = next_decoded_val {
                    dict.insert(key, self.decode()?);
                } else {
                    return Err(BencodeError::parse_error(
                        "Invalid encoding, dict's key must be string",
                    ));
                };
            }
        }
    }
}

#[allow(dead_code)]
fn decode_bencoded_value(encoded_value: &str) -> serde_json::Value {
    let mut decoder = bencode::BencodeDecoder::new(encoded_value);
    decoder.decode().unwrap()
}
