use anyhow::Context;
use serde_json;
use std::env;
// use serde_bencode

#[derive(Debug)]
enum BencodeError {
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

enum BencodeType {
    BtString(String),
    BtNumber(i64),
    BtLists(Vec<serde_json::Value>),
    BtDictionary(serde_json::Value),
}

struct BencodeDecoder<'a> {
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
            Ok(BencodeType::BtString(decoded_str)) => Ok(serde_json::Value::String(decoded_str)),
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
            BencodeError::parse_error("Invalid encoding format for string, invalid encoded length")
        })?;
        let end = colon_index + 1 + number as usize;
        let string = &cur_range[colon_index + 1..end];
        self.start_pos += end;
        return Ok(BencodeType::BtString(string.to_string()));
    }

    fn validate_bt_number(number_string: &str) -> Result<(), BencodeError> {
        if number_string.len() <= 1 {
            return Ok(());
        }
        let mut chars = number_string.chars();
        // Retrieve first two characters or return an error
        let first_char = chars
            .next()
            .expect("Expected number <i..e> to have at least first char");
        let second_char = chars
            .next()
            .expect("Expected number <i..e> to have at least second char");
        // Validate based on the first character
        match first_char {
            '-' => {
                if second_char == '0' {
                    return Err(BencodeError::parse_error(
                        "Invalid number format, `i-0` is invalid",
                    ));
                }
            }
            '0' => {
                if second_char != 'e' {
                    return Err(BencodeError::parse_error(&format!(
                        "Invalid number format, `i0{}` is invalid",
                        second_char
                    )));
                }
            }
            _ => {} // If the first character is valid, do nothing
        }
        Ok(())
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
        BencodeDecoder::validate_bt_number(number_string)?;
        let number = number_string.parse::<i64>().map_err(|_| {
            BencodeError::parse_error("Invalid encoding format for number, invalid encoded number")
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

#[allow(dead_code)]
fn decode_bencoded_value(encoded_value: &str) -> serde_json::Value {
    let mut decoder = BencodeDecoder::new(encoded_value);
    decoder.decode().unwrap()
}

// Usage: your_bittorrent.sh decode "<encoded_value>"
fn main() {
    let args: Vec<String> = env::args().collect();
    let command = &args[1];

    if command == "decode" {
        let encoded_value = &args[2];
        let decoded_value = decode_bencoded_value(encoded_value);
        println!("{}", decoded_value.to_string());
    } else {
        println!("unknown command: {}", args[1])
    }
}
