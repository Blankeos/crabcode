use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tiktoken_rs::{
    get_bpe_from_tokenizer,
    tokenizer::{get_tokenizer, Tokenizer},
    CoreBPE,
};

const TAIL_CONTEXT_CHARS: usize = 256;

#[derive(Clone)]
pub struct StreamingTokenCounter {
    encoder: TokenEncoder,
    total_tokens: usize,
    tail_text: String,
    tail_tokens: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bpe(counter: &StreamingTokenCounter) -> &Arc<CoreBPE> {
        match &counter.encoder {
            TokenEncoder::Tiktoken(bpe) => bpe,
            TokenEncoder::Approximate => panic!("expected a tiktoken encoder"),
        }
    }

    #[test]
    fn counters_share_encoders_across_model_ids() {
        let first = StreamingTokenCounter::new("gpt-4o");
        for model in ["gpt-4o", "gpt-5", "gpt-4o-2024-05-13", "openai/gpt-5"] {
            let other = StreamingTokenCounter::new(model);
            assert!(Arc::ptr_eq(bpe(&first), bpe(&other)), "{model}");
        }
        let older = StreamingTokenCounter::new("gpt-4");
        let other = StreamingTokenCounter::new("gpt-3.5-turbo");
        assert!(Arc::ptr_eq(bpe(&older), bpe(&other)));
        assert!(!Arc::ptr_eq(bpe(&first), bpe(&older)));
    }

    #[test]
    fn fallback_routing_preserves_modern_and_older_encodings() {
        let modern = StreamingTokenCounter::new("gpt-4o");
        let older = StreamingTokenCounter::new("gpt-4");
        for model in [
            "openai/GPT-5",
            "openai/gpt-4o",
            "openai/gpt-4.1",
            "o1-custom",
            "o3-custom",
            "o4-custom",
            "provider/o1-custom",
            "provider/o3-custom",
            "provider/o4-custom",
        ] {
            assert!(
                Arc::ptr_eq(bpe(&modern), bpe(&StreamingTokenCounter::new(model))),
                "{model}"
            );
        }
        for model in ["unknown-model", "anthropic/claude", "provider/gpt-4", ""] {
            assert!(
                Arc::ptr_eq(bpe(&older), bpe(&StreamingTokenCounter::new(model))),
                "{model}"
            );
        }
        // Known legacy models must not be routed through the cl100k fallback.
        let legacy = StreamingTokenCounter::new("text-davinci-003");
        let code = StreamingTokenCounter::new("code-davinci-002");
        assert!(Arc::ptr_eq(bpe(&legacy), bpe(&code)));
        assert!(!Arc::ptr_eq(bpe(&legacy), bpe(&older)));
    }

    #[test]
    fn shared_encoder_keeps_counter_state_independent() {
        let mut first = StreamingTokenCounter::new("gpt-4o");
        let mut second = StreamingTokenCounter::new("gpt-5");
        first.add_text("Hello, world!");
        assert_eq!(second.total_tokens(), 0);
        assert!(second.tail_text.is_empty());
        second.add_text("An independent message.");
        let second_total = second.total_tokens();
        let second_tail = second.tail_text.clone();
        let second_tail_tokens = second.tail_tokens;
        first.reset();
        assert_eq!(first.total_tokens(), 0);
        assert!(first.tail_text.is_empty());
        assert_eq!(first.tail_tokens, 0);
        assert_eq!(second.total_tokens(), second_total);
        assert_eq!(second.tail_text, second_tail);
        assert_eq!(second.tail_tokens, second_tail_tokens);
    }

    #[test]
    fn cached_counts_match_fresh_encoders() {
        for (model, tokenizer) in [
            ("gpt-4o", Tokenizer::O200kBase),
            ("provider/gpt-5", Tokenizer::O200kBase),
            ("gpt-4", Tokenizer::Cl100kBase),
            ("unknown-model", Tokenizer::Cl100kBase),
            ("text-davinci-003", Tokenizer::P50kBase),
        ] {
            let fresh = get_bpe_from_tokenizer(tokenizer).unwrap();
            let mut counter = StreamingTokenCounter::new(model);
            let mut text = String::new();
            for chunk in ["Hello", ", world!", "", " 日本語 🦀", "\nfn main() {}"] {
                text.push_str(chunk);
                assert_eq!(
                    counter.add_text(chunk),
                    fresh.encode_ordinary(&text).len(),
                    "{model}"
                );
            }
        }
    }

    #[test]
    fn approximate_encoder_behavior_is_unchanged() {
        let mut counter = StreamingTokenCounter {
            encoder: TokenEncoder::Approximate,
            total_tokens: 0,
            tail_text: String::new(),
            tail_tokens: 0,
        };
        assert_eq!(counter.add_text("abcde"), 2);
        assert_eq!(counter.add_text("🦀"), 3);
        assert_eq!(counter.add_text(""), 3);
        counter.reset();
        assert_eq!(counter.total_tokens(), 0);
    }
}

#[derive(Clone)]
enum TokenEncoder {
    Tiktoken(Arc<CoreBPE>),
    Approximate,
}

impl StreamingTokenCounter {
    pub fn new(model: &str) -> Self {
        let encoder = get_tokenizer(model)
            .and_then(cached_encoder)
            .or_else(|| fallback_encoder(model))
            .unwrap_or(TokenEncoder::Approximate);

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

fn cached_encoder(tokenizer: Tokenizer) -> Option<TokenEncoder> {
    // Key by the finite encoding set, not user-supplied model IDs. Cache failures
    // too, and serialize initialization so concurrent counters build each BPE once.
    static ENCODERS: OnceLock<Mutex<HashMap<Tokenizer, Option<TokenEncoder>>>> = OnceLock::new();
    let mut encoders = ENCODERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    encoders
        .entry(tokenizer)
        .or_insert_with(|| {
            get_bpe_from_tokenizer(tokenizer)
                .map(|bpe| TokenEncoder::Tiktoken(Arc::new(bpe)))
                .ok()
        })
        .clone()
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

    cached_encoder(if use_o200k {
        Tokenizer::O200kBase
    } else {
        Tokenizer::Cl100kBase
    })
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
