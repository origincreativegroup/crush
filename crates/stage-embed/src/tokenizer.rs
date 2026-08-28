//! The OpenAI CLIP byte-pair tokenizer used by ViT-B/32.

use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use anyhow::{ensure, Context};
use flate2::read::GzDecoder;
use regex::Regex;

pub const CONTEXT_LENGTH: usize = 77;
pub const START_OF_TEXT: i64 = 49_406;
pub const END_OF_TEXT: i64 = 49_407;
const MERGE_COUNT: usize = 49_152 - 256 - 2;

pub struct ClipTokenizer {
    byte_encoder: [String; 256],
    encoder: HashMap<String, i64>,
    bpe_ranks: HashMap<(String, String), usize>,
    cache: HashMap<String, Vec<String>>,
    pattern: Regex,
}

impl ClipTokenizer {
    pub fn from_gzip(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let byte_pairs = byte_to_unicode();
        let mut byte_encoder: [String; 256] = std::array::from_fn(|_| String::new());
        let mut vocab = Vec::with_capacity(49_408);
        for &(byte, scalar) in &byte_pairs {
            let symbol = scalar.to_string();
            byte_encoder[usize::from(byte)] = symbol.clone();
            vocab.push(symbol);
        }
        vocab.extend(
            vocab
                .clone()
                .into_iter()
                .map(|token| format!("{token}</w>")),
        );

        let file = File::open(path)
            .with_context(|| format!("failed to open CLIP BPE vocabulary {}", path.display()))?;
        let mut lines = BufReader::new(GzDecoder::new(file)).lines();
        // OpenCLIP treats the first line as an opaque header. The packaged upstream file includes
        // its historical filename before `#version`, so intentionally do not parse it.
        let _header = lines.next().context("CLIP BPE vocabulary is empty")??;
        let mut merges = Vec::with_capacity(MERGE_COUNT);
        for line in lines.take(MERGE_COUNT) {
            let line = line?;
            let mut pieces = line.split_whitespace();
            let left = pieces.next().context("invalid empty CLIP BPE merge")?;
            let right = pieces.next().context("invalid one-part CLIP BPE merge")?;
            ensure!(pieces.next().is_none(), "invalid CLIP BPE merge: {line}");
            merges.push((left.to_owned(), right.to_owned()));
        }
        ensure!(
            merges.len() == MERGE_COUNT,
            "CLIP BPE vocabulary has {} merges; expected {MERGE_COUNT}",
            merges.len()
        );
        vocab.extend(merges.iter().map(|(left, right)| format!("{left}{right}")));
        vocab.push("<start_of_text>".to_owned());
        vocab.push("<end_of_text>".to_owned());
        ensure!(vocab.len() == 49_408, "CLIP vocabulary has wrong size");

        let encoder = vocab
            .into_iter()
            .enumerate()
            .map(|(index, token)| (token, index as i64))
            .collect();
        let bpe_ranks = merges
            .into_iter()
            .enumerate()
            .map(|(rank, pair)| (pair, rank))
            .collect();
        let pattern = Regex::new(
            r"(?i)<start_of_text>|<end_of_text>|'s|'t|'re|'ve|'m|'ll|'d|[\p{L}]+|[\p{N}]|[^\s\p{L}\p{N}]+",
        )?;
        Ok(Self {
            byte_encoder,
            encoder,
            bpe_ranks,
            cache: HashMap::new(),
            pattern,
        })
    }

    pub fn encode(&mut self, text: &str) -> anyhow::Result<[i64; CONTEXT_LENGTH]> {
        let cleaned = whitespace_clean(&basic_clean(text)).to_lowercase();
        let mut ids = Vec::with_capacity(CONTEXT_LENGTH);
        ids.push(START_OF_TEXT);
        let tokens = self
            .pattern
            .find_iter(&cleaned)
            .map(|found| found.as_str().to_owned())
            .collect::<Vec<_>>();
        for token in tokens {
            let encoded = token
                .as_bytes()
                .iter()
                .map(|byte| self.byte_encoder[usize::from(*byte)].as_str())
                .collect::<String>();
            for piece in self.bpe(&encoded) {
                ids.push(
                    *self
                        .encoder
                        .get(&piece)
                        .with_context(|| format!("CLIP BPE produced unknown token {piece:?}"))?,
                );
            }
        }
        ids.truncate(CONTEXT_LENGTH - 1);
        ids.push(END_OF_TEXT);
        let mut padded = [0_i64; CONTEXT_LENGTH];
        padded[..ids.len()].copy_from_slice(&ids);
        Ok(padded)
    }

    fn bpe(&mut self, token: &str) -> Vec<String> {
        if let Some(cached) = self.cache.get(token) {
            return cached.clone();
        }
        let mut word = token
            .chars()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        if let Some(last) = word.last_mut() {
            last.push_str("</w>");
        }
        while let Some((_, pair)) = word
            .windows(2)
            .filter_map(|window| {
                let pair = (window[0].clone(), window[1].clone());
                self.bpe_ranks.get(&pair).copied().map(|rank| (rank, pair))
            })
            .min_by_key(|(rank, _)| *rank)
        {
            let mut merged = Vec::with_capacity(word.len());
            let mut index = 0;
            while index < word.len() {
                if index + 1 < word.len() && word[index] == pair.0 && word[index + 1] == pair.1 {
                    merged.push(format!("{}{}", word[index], word[index + 1]));
                    index += 2;
                } else {
                    merged.push(word[index].clone());
                    index += 1;
                }
            }
            word = merged;
            if word.len() == 1 {
                break;
            }
        }
        self.cache.insert(token.to_owned(), word.clone());
        word
    }
}

fn byte_to_unicode() -> Vec<(u8, char)> {
    let mut bytes = (b'!'..=b'~')
        .chain(0xA1..=0xAC)
        .chain(0xAE..=0xFF)
        .collect::<Vec<_>>();
    let mut scalars = bytes
        .iter()
        .map(|&byte| u32::from(byte))
        .collect::<Vec<_>>();
    let mut extra = 0_u32;
    for byte in 0_u8..=u8::MAX {
        if !bytes.contains(&byte) {
            bytes.push(byte);
            scalars.push(256 + extra);
            extra += 1;
        }
    }
    bytes
        .into_iter()
        .zip(scalars)
        .map(|(byte, scalar)| {
            (
                byte,
                char::from_u32(scalar).expect("CLIP byte scalar is valid"),
            )
        })
        .collect()
}

fn basic_clean(text: &str) -> String {
    html_unescape_once(&html_unescape_once(text))
        .trim()
        .to_owned()
}

fn whitespace_clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn html_unescape_once(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut remainder = text;
    while let Some(start) = remainder.find('&') {
        output.push_str(&remainder[..start]);
        remainder = &remainder[start..];
        let Some(end) = remainder.find(';').filter(|end| *end <= 32) else {
            output.push('&');
            remainder = &remainder[1..];
            continue;
        };
        let entity = &remainder[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some('\u{00A0}'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|digits| u32::from_str_radix(digits, 16).ok())
                .and_then(char::from_u32)
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|digits| digits.parse::<u32>().ok())
                        .and_then(char::from_u32)
                }),
        };
        if let Some(character) = decoded {
            output.push(character);
        } else {
            output.push_str(&remainder[..=end]);
        }
        remainder = &remainder[end + 1..];
    }
    output.push_str(remainder);
    output
}
