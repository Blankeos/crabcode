use std::sync::Arc;

use tiktoken_rs::{cl100k_base, get_bpe_from_model, o200k_base, CoreBPE};

const TAIL_CONTEXT_CHARS: usize = 256;

#[derive(Clone)]
pub struct StreamingTokenCounter {
    encoder: TokenEncoder,
    total_tokens: usize,
    tail_text: String,
    tail_tokens: usize,
}

#[derive(Clone)]
enum TokenEncoder {
    Tiktoken(Arc<CoreBPE>),
    Approximate,
}

impl StreamingTokenCounter {
    pub fn new(model: &str) -> Self {
        let encoder = match get_bpe_from_model(model) {
            Ok(bpe) => TokenEncoder::Tiktoken(Arc::new(bpe)),
            Err(_) => fallback_encoder(model).unwrap_or(TokenEncoder::Approximate),
        };

        Self {
            encoder,
            total_tokens: 0,
            tail_text: String::new(),
            tail_tokens: 0,
        }
    }

    pub fn reset(&mut self) {
        self.total_tokens = 0;
        self.tail_text.clear();
        self.tail_tokens = 0;
    }

    pub fn add_text(&mut self, text: &str) -> usize {
        if text.is_empty() {
            return self.total_tokens;
        }

        match &self.encoder {
            TokenEncoder::Tiktoken(bpe) => {
                let combined = format!("{}{}", self.tail_text, text);
                let combined_tokens = bpe.encode_ordinary(&combined).len();
                self.total_tokens =
                    self.total_tokens.saturating_sub(self.tail_tokens) + combined_tokens;

                self.tail_text = take_last_chars(&combined, TAIL_CONTEXT_CHARS);
                self.tail_tokens = bpe.encode_ordinary(&self.tail_text).len();
            }
            TokenEncoder::Approximate => {
                self.total_tokens = self.total_tokens.saturating_add(approximate_tokens(text));
            }
        }

        self.total_tokens
    }

    pub fn total_tokens(&self) -> usize {
        self.total_tokens
    }
}

impl std::fmt::Debug for StreamingTokenCounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingTokenCounter")
            .field("total_tokens", &self.total_tokens)
            .field("tail_len", &self.tail_text.chars().count())
            .finish()
    }
}

fn fallback_encoder(model: &str) -> Option<TokenEncoder> {
    let model_lower = model.to_lowercase();
    let use_o200k = model_lower.contains("gpt-5")
        || model_lower.contains("gpt-4o")
        || model_lower.contains("gpt-4.1")
        || model_lower.starts_with("o1")
        || model_lower.starts_with("o3")
        || model_lower.starts_with("o4")
        || model_lower.contains("o1-")
        || model_lower.contains("o3-")
        || model_lower.contains("o4-");

    if use_o200k {
        return o200k_base()
            .map(|bpe| TokenEncoder::Tiktoken(Arc::new(bpe)))
            .ok();
    }

    cl100k_base()
        .map(|bpe| TokenEncoder::Tiktoken(Arc::new(bpe)))
        .ok()
}

fn approximate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    (chars.saturating_add(3)) / 4
}

fn take_last_chars(text: &str, max_chars: usize) -> String {
    let mut chars: Vec<char> = text.chars().rev().take(max_chars).collect();
    chars.reverse();
    chars.into_iter().collect()
}
