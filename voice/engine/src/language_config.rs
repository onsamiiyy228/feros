//! Provider-agnostic language and TTS model configuration.
//!
//! This module is the **single source of truth** for language support and TTS
//! model/voice capability across the stack. It is exposed to Python via
//! PyO3 so that:
//!
//!   - The Python admin API serves `GET /api/agents/languages` from this data.
//!   - The Python admin API serves `GET /api/agents/tts-models` from this data.
//!   - The frontend language dropdown and model picker are populated dynamically.
//!   - Greeting regeneration and LLM language-instruction injection use the
//!     label from here, not a separate Python-side lookup table.
//!   - Session start emits a `tracing::warn!` when provider+model cannot
//!     synthesize the configured language.
//!
//! **No language configuration belongs in the database** — this is static
//! provider metadata, not user data.
//!
//! ## Adding a new language
//!
//! 1. Add one row to `SUPPORTED_LANGUAGES`.
//! 2. Add the code to `supported_languages` in every `TtsModelSpec` row that
//!    supports it (multilingual models).
//! 3. Run `cargo test -p voice-engine` — catalog integrity tests will catch
//!    any stale codes.
//!
//! The PyO3 export, API route, and frontend dropdown all update automatically
//! on the next build.
//!
//! ## Adding a new TTS model
//!
//! Add one row to `TTS_MODEL_CATALOG`. See the doc-comment on `TtsModelSpec`
//! for field semantics and step-by-step instructions.
//!
//! ## Adding a new TTS provider
//!
//! 1. Add a `<provider>_code` field to `LanguageOption` and populate the
//!    `SUPPORTED_LANGUAGES` table.
//! 2. Add lookup helpers following the pattern of `elevenlabs_language_code` /
//!    `cartesia_language_code`.
//! 3. Add one or more `TtsModelSpec` rows to `TTS_MODEL_CATALOG`.

// ── Types ─────────────────────────────────────────────────────────────────────

/// Metadata for a single language across all TTS/STT providers.
///
/// `code` is the canonical ISO 639-1 base code (no region suffix) stored in
/// the DB and threaded through the entire stack. Provider-specific fields
/// exist so we can diverge when a provider uses a non-standard identifier.
/// Today all providers accept the same base codes, but keeping the fields
/// separate makes future divergence a one-line change.
#[derive(Debug, Clone, Copy)]
pub struct LanguageOption {
    /// Canonical code: ISO 639-1 base code, e.g. `"en"`, `"zh"`, `"pt"`.
    /// Stored in `agent_versions.config_json` and passed through all layers.
    pub code: &'static str,
    /// Human-readable label for the frontend dropdown (surfaced via PyO3).
    pub label: &'static str,
    /// ElevenLabs `language_code` URL param value (multilingual models only).
    pub elevenlabs_code: &'static str,
    /// Deepgram `language` URL query param value (BCP-47 base code).
    pub deepgram_code: &'static str,
    /// Cartesia `language` JSON field value.
    pub cartesia_code: &'static str,
}

/// A curated default voice for a specific TTS model and (optionally) language.
///
/// For **multilingual models** (Cartesia sonic-2/3, ElevenLabs flash/turbo
/// v2.5), a single voice works for any language — only the `language` /
/// `language_code` API parameter changes the output language. These models
/// carry exactly **one** `LanguageVoice` entry with `language_code = "en"` as
/// the universal starting default. Contributors picking a voice for a
/// multilingual model should choose one that sounds natural in English as the
/// cold-start default; users can change it via the voice picker.
///
/// For **voice-encodes-language providers** (Deepgram Aura), the voice name
/// carries the language identifier (e.g. `aura-2-zeus-en`). These models have
/// **one entry per supported language** so the engine can auto-select the
/// correct voice when language is set.
#[derive(Debug, Clone, Copy)]
pub struct LanguageVoice {
    /// Language code from `SUPPORTED_LANGUAGES.code`. Use `"en"` for the
    /// universal default in multilingual-model entries.
    pub language_code: &'static str,
    /// Provider-specific opaque voice identifier.
    pub voice_id: &'static str,
    /// Human-readable name shown in the UI voice picker.
    pub voice_label: &'static str,
}

/// A curated TTS model entry — defines language support and default voices.
///
/// ## Adding a new model (contributor guide)
///
/// 1. Add a row to `TTS_MODEL_CATALOG`.
/// 2. Set `provider` to the exact `tts_provider` slug in agent config
///    (e.g. `"cartesia-ws"`, `"elevenlabs-ws"`, `"deepgram"`).
/// 3. Set `model_id` to the string sent to the provider API.
/// 4. Populate `supported_languages` from `SUPPORTED_LANGUAGES.code` values.
///    Only list languages the model actually supports to avoid silent failures.
/// 5. **Multilingual model**: add exactly **one** `LanguageVoice { language_code: "en", … }`.
/// 6. **Voice-encodes-language provider** (Deepgram Aura): add one `LanguageVoice`
///    per supported language with the matching voice ID.
/// 7. Run `cargo test -p voice-engine` — catalog integrity tests catch stale codes.
#[derive(Debug, Clone, Copy)]
pub struct TtsModelSpec {
    /// Provider slug — must match the `tts_provider` field in agent config.
    ///
    /// Valid values: `"cartesia"`, `"cartesia-ws"`, `"elevenlabs"`,
    /// `"elevenlabs-ws"`, `"deepgram"`, `"deepgram-ws"`, `"openai"`, `"groq"`.
    pub provider: &'static str,

    /// Model identifier sent verbatim to the provider API.
    ///
    /// For Deepgram Aura, the voice name IS the model (e.g. `"aura-2-zeus-en"`);
    /// use the family name (e.g. `"aura-2"`) here and carry per-language voice
    /// IDs in `language_voices`.
    pub model_id: &'static str,

    /// Human-readable label for UI dropdowns (e.g. `"Cartesia Sonic 2"`).
    pub label: &'static str,

    /// Subset of `SUPPORTED_LANGUAGES.code` values this model can synthesize.
    ///
    /// Use `&[]` only to mark a model that should never appear in the catalog
    /// — omit such models entirely instead.
    pub supported_languages: &'static [&'static str],

    /// Curated voice defaults. See `LanguageVoice` for semantics.
    ///
    /// Use `default_voice_for_language()` rather than indexing directly.
    pub language_voices: &'static [LanguageVoice],
}

impl TtsModelSpec {
    /// Return the recommended voice ID for `lang`.
    ///
    /// For multilingual models (one entry with `language_code = "en"`), this
    /// always returns that single default voice regardless of `lang`.
    ///
    /// For voice-encodes-language models (Deepgram), returns the language-
    /// specific voice if present, falling back to the first entry.
    ///
    /// Returns `None` if `language_voices` is empty.
    pub fn default_voice_for_language(&self, lang: &str) -> Option<&'static str> {
        let base = normalize_to_base_code(lang);
        self.language_voices
            .iter()
            .find(|v| v.language_code == base)
            .or_else(|| self.language_voices.first())
            .map(|v| v.voice_id)
    }

    /// True if this model can synthesize `lang`.
    pub fn supports_language(&self, lang: &str) -> bool {
        if lang.is_empty() || lang == "en" {
            return true;
        }
        let base = normalize_to_base_code(lang);
        self.supported_languages.contains(&base)
    }
}

/// A curated STT model entry — defines language support.
#[derive(Debug, Clone, Copy)]
pub struct SttModelSpec {
    pub provider: &'static str,
    pub model_id: &'static str,
    pub label: &'static str,
    pub supported_languages: &'static [&'static str],
}

impl SttModelSpec {
    /// True if this model can transcribe `lang`.
    pub fn supports_language(&self, lang: &str) -> bool {
        if lang.is_empty() || lang == "en" {
            return true;
        }
        let base = normalize_to_base_code(lang);
        self.supported_languages.contains(&base)
    }
}

// ── Constants ─────────────────────────────────────────────────────────────────

/// Curated list of languages shown in the UI and supported by our TTS/STT stack.
///
/// This is a deliberate subset of what individual providers support:
///   - Cartesia map has 36 languages
///   - ElevenLabs map has ~32 languages
///
/// Codes not in this list degrade gracefully: lookup helpers return `""`
/// which causes callers to omit the language parameter → provider defaults.
pub const SUPPORTED_LANGUAGES: &[LanguageOption] = &[
    LanguageOption {
        code: "en",
        label: "English",
        elevenlabs_code: "en",
        deepgram_code: "en",
        cartesia_code: "en",
    },
    LanguageOption {
        code: "es",
        label: "Spanish",
        elevenlabs_code: "es",
        deepgram_code: "es",
        cartesia_code: "es",
    },
    LanguageOption {
        code: "fr",
        label: "French",
        elevenlabs_code: "fr",
        deepgram_code: "fr",
        cartesia_code: "fr",
    },
    LanguageOption {
        code: "de",
        label: "German",
        elevenlabs_code: "de",
        deepgram_code: "de",
        cartesia_code: "de",
    },
    LanguageOption {
        code: "pt",
        label: "Portuguese",
        elevenlabs_code: "pt",
        deepgram_code: "pt",
        cartesia_code: "pt",
    },
    LanguageOption {
        code: "it",
        label: "Italian",
        elevenlabs_code: "it",
        deepgram_code: "it",
        cartesia_code: "it",
    },
    LanguageOption {
        code: "ja",
        label: "Japanese",
        elevenlabs_code: "ja",
        deepgram_code: "ja",
        cartesia_code: "ja",
    },
    LanguageOption {
        code: "ko",
        label: "Korean",
        elevenlabs_code: "ko",
        deepgram_code: "ko",
        cartesia_code: "ko",
    },
    LanguageOption {
        code: "zh",
        label: "Chinese (Mandarin, Simplified)",
        elevenlabs_code: "zh",
        deepgram_code: "zh",
        cartesia_code: "zh",
    },
    LanguageOption {
        code: "zh-TW",
        label: "Chinese (Mandarin, Traditional)",
        elevenlabs_code: "zh",
        deepgram_code: "zh-TW",
        cartesia_code: "zh",
    },
    LanguageOption {
        code: "zh-HK",
        label: "Chinese (Cantonese, Traditional)",
        elevenlabs_code: "zh",
        deepgram_code: "zh-HK",
        cartesia_code: "zh",
    },
    LanguageOption {
        code: "ar",
        label: "Arabic",
        elevenlabs_code: "ar",
        deepgram_code: "ar",
        cartesia_code: "ar",
    },
    LanguageOption {
        code: "hi",
        label: "Hindi",
        elevenlabs_code: "hi",
        deepgram_code: "hi",
        cartesia_code: "hi",
    },
    LanguageOption {
        code: "nl",
        label: "Dutch",
        elevenlabs_code: "nl",
        deepgram_code: "nl",
        cartesia_code: "nl",
    },
    LanguageOption {
        code: "ru",
        label: "Russian",
        elevenlabs_code: "ru",
        deepgram_code: "ru",
        cartesia_code: "ru",
    },
    LanguageOption {
        code: "pl",
        label: "Polish",
        elevenlabs_code: "pl",
        deepgram_code: "pl",
        cartesia_code: "pl",
    },
    LanguageOption {
        code: "sv",
        label: "Swedish",
        elevenlabs_code: "sv",
        deepgram_code: "sv",
        cartesia_code: "sv",
    },
];

/// ElevenLabs model IDs that accept the `language_code` URL parameter.
///
/// Only these "v2_5" models support the `language_code` URL param.
/// All older models (eleven_multilingual_v2, eleven_turbo_v2, eleven_flash_v2)
/// encode language implicitly via the chosen voice — sending `language_code`
/// to them is silently ignored or may cause an error.
///
/// Source: verified against ElevenLabs API documentation.
pub const ELEVENLABS_MULTILINGUAL_MODELS: &[&str] = &["eleven_flash_v2_5", "eleven_turbo_v2_5"];

// ── TTS model catalog ─────────────────────────────────────────────────────────

/// Curated TTS model catalog — the canonical source for model/language/voice
/// compatibility across the stack.
///
/// # Design decisions
///
/// - **Prefer "omni" multilingual models.** Cartesia sonic-2/3 and ElevenLabs
///   flash/turbo v2.5 support all target languages via an API parameter; they
///   appear once with the full language list. No per-language model variants
///   are needed for them.
/// - **Voice-encodes-language providers (Deepgram Aura)** carry one entry per
///   supported language in `language_voices`. Add a new row in this catalog if
///   a new Aura language family is released.
/// - **English-only models** (e.g. `sonic-english`) are NOT listed. They are
///   implicitly handled by the `tts_model_supports_language()` helper returning
///   `false` for any non-English language on an unknown model.
/// - **Voice IDs are opaque.** We maintain one curated default per
///   (model, language) for initial agent setup. Users override via the voice
///   picker which proxies `GET /api/agents/voices/{provider}`.
/// - **WS and HTTP variants** are listed separately because they are distinct
///   `tts_provider` values in agent config. API consumers should filter by the
///   agent's actual `tts_provider`.
///
/// # Adding a new model — see `TtsModelSpec` for the step-by-step guide.
pub const TTS_MODEL_CATALOG: &[TtsModelSpec] = &[
    // ── Cartesia ─────────────────────────────────────────────────────────────
    // sonic-2 and sonic-3 are Cartesia's multilingual models.
    // The `language` JSON field in every request selects output language;
    // the voice ID is orthogonal — any Cartesia voice works with any language.
    // Source: https://docs.cartesia.ai/api-reference/tts/tts
    TtsModelSpec {
        provider: "cartesia-ws",
        model_id: "sonic-2",
        label: "Cartesia Sonic 2",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
        // One universal default voice (multilingual model — voice ≠ language).
        language_voices: &[LanguageVoice {
            language_code: "en",
            voice_id: "694f9389-aac1-45b6-b726-9d9369183238",
            voice_label: "Barbershop Man",
        }],
    },
    TtsModelSpec {
        provider: "cartesia-ws",
        model_id: "sonic-3",
        label: "Cartesia Sonic 3",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
        language_voices: &[LanguageVoice {
            language_code: "en",
            voice_id: "694f9389-aac1-45b6-b726-9d9369183238",
            voice_label: "Barbershop Man",
        }],
    },
    // HTTP variants — same model IDs, mirrored for non-WS usage.
    TtsModelSpec {
        provider: "cartesia",
        model_id: "sonic-2",
        label: "Cartesia Sonic 2",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
        language_voices: &[LanguageVoice {
            language_code: "en",
            voice_id: "694f9389-aac1-45b6-b726-9d9369183238",
            voice_label: "Barbershop Man",
        }],
    },
    TtsModelSpec {
        provider: "cartesia",
        model_id: "sonic-3",
        label: "Cartesia Sonic 3",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
        language_voices: &[LanguageVoice {
            language_code: "en",
            voice_id: "694f9389-aac1-45b6-b726-9d9369183238",
            voice_label: "Barbershop Man",
        }],
    },
    // ── ElevenLabs ───────────────────────────────────────────────────────────
    // eleven_flash_v2_5 and eleven_turbo_v2_5 accept the `language_code` URL
    // param (see ELEVENLABS_MULTILINGUAL_MODELS). The voice_id is language-
    // agnostic; callers must also ensure the model is in ELEVENLABS_MULTILINGUAL_MODELS
    // for language_code to be applied.
    // Source: https://elevenlabs.io/docs/capabilities/supported-languages
    TtsModelSpec {
        provider: "elevenlabs-ws",
        model_id: "eleven_flash_v2_5",
        label: "ElevenLabs Flash v2.5",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
        language_voices: &[LanguageVoice {
            language_code: "en",
            voice_id: "21m00Tcm4TlvDq8ikWAM",
            voice_label: "Rachel",
        }],
    },
    TtsModelSpec {
        provider: "elevenlabs-ws",
        model_id: "eleven_turbo_v2_5",
        label: "ElevenLabs Turbo v2.5",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
        language_voices: &[LanguageVoice {
            language_code: "en",
            voice_id: "21m00Tcm4TlvDq8ikWAM",
            voice_label: "Rachel",
        }],
    },
    // HTTP variants.
    TtsModelSpec {
        provider: "elevenlabs",
        model_id: "eleven_flash_v2_5",
        label: "ElevenLabs Flash v2.5",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
        language_voices: &[LanguageVoice {
            language_code: "en",
            voice_id: "21m00Tcm4TlvDq8ikWAM",
            voice_label: "Rachel",
        }],
    },
    TtsModelSpec {
        provider: "elevenlabs",
        model_id: "eleven_turbo_v2_5",
        label: "ElevenLabs Turbo v2.5",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
        language_voices: &[LanguageVoice {
            language_code: "en",
            voice_id: "21m00Tcm4TlvDq8ikWAM",
            voice_label: "Rachel",
        }],
    },
    // ── Deepgram Aura ────────────────────────────────────────────────────────
    // Deepgram Aura voices encode language in the voice name (aura-2-zeus-en).
    // There is no separate "model" API param for TTS — the voice string sent
    // as `model` IS the combined model+voice+language identifier.
    //
    // Aura 2: 7 languages — en, es, de, fr, nl, it, ja
    // Aura 1: English only (legacy)
    // Source: https://developers.deepgram.com/docs/tts-models
    TtsModelSpec {
        provider: "deepgram",
        model_id: "aura-2",
        label: "Deepgram Aura 2",
        supported_languages: &["en", "es", "de", "fr", "nl", "it", "ja"],
        language_voices: &[
            // English
            LanguageVoice {
                language_code: "en",
                voice_id: "aura-2-thalia-en",
                voice_label: "Thalia (F)",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "aura-2-zeus-en",
                voice_label: "Zeus (M)",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "aura-2-luna-en",
                voice_label: "Luna (F)",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "aura-2-apollo-en",
                voice_label: "Apollo (M)",
            },
            // Spanish
            LanguageVoice {
                language_code: "es",
                voice_id: "aura-2-celeste-es",
                voice_label: "Celeste (F)",
            },
            LanguageVoice {
                language_code: "es",
                voice_id: "aura-2-nestor-es",
                voice_label: "Nestor (M)",
            },
            LanguageVoice {
                language_code: "es",
                voice_id: "aura-2-estrella-es",
                voice_label: "Estrella (F)",
            },
            // German
            LanguageVoice {
                language_code: "de",
                voice_id: "aura-2-julius-de",
                voice_label: "Julius (M)",
            },
            LanguageVoice {
                language_code: "de",
                voice_id: "aura-2-viktoria-de",
                voice_label: "Viktoria (F)",
            },
            // French
            LanguageVoice {
                language_code: "fr",
                voice_id: "aura-2-agathe-fr",
                voice_label: "Agathe (F)",
            },
            LanguageVoice {
                language_code: "fr",
                voice_id: "aura-2-hector-fr",
                voice_label: "Hector (M)",
            },
            // Dutch
            LanguageVoice {
                language_code: "nl",
                voice_id: "aura-2-rhea-nl",
                voice_label: "Rhea (F)",
            },
            LanguageVoice {
                language_code: "nl",
                voice_id: "aura-2-sander-nl",
                voice_label: "Sander (M)",
            },
            // Italian
            LanguageVoice {
                language_code: "it",
                voice_id: "aura-2-livia-it",
                voice_label: "Livia (F)",
            },
            LanguageVoice {
                language_code: "it",
                voice_id: "aura-2-dionisio-it",
                voice_label: "Dionisio (M)",
            },
            // Japanese
            LanguageVoice {
                language_code: "ja",
                voice_id: "aura-2-fujin-ja",
                voice_label: "Fujin (M)",
            },
            LanguageVoice {
                language_code: "ja",
                voice_id: "aura-2-izanami-ja",
                voice_label: "Izanami (F)",
            },
        ],
    },
    TtsModelSpec {
        provider: "deepgram-ws",
        model_id: "aura-2",
        label: "Deepgram Aura 2",
        supported_languages: &["en", "es", "de", "fr", "nl", "it", "ja"],
        language_voices: &[
            // English
            LanguageVoice {
                language_code: "en",
                voice_id: "aura-2-thalia-en",
                voice_label: "Thalia (F)",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "aura-2-zeus-en",
                voice_label: "Zeus (M)",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "aura-2-luna-en",
                voice_label: "Luna (F)",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "aura-2-apollo-en",
                voice_label: "Apollo (M)",
            },
            // Spanish
            LanguageVoice {
                language_code: "es",
                voice_id: "aura-2-celeste-es",
                voice_label: "Celeste (F)",
            },
            LanguageVoice {
                language_code: "es",
                voice_id: "aura-2-nestor-es",
                voice_label: "Nestor (M)",
            },
            LanguageVoice {
                language_code: "es",
                voice_id: "aura-2-estrella-es",
                voice_label: "Estrella (F)",
            },
            // German
            LanguageVoice {
                language_code: "de",
                voice_id: "aura-2-julius-de",
                voice_label: "Julius (M)",
            },
            LanguageVoice {
                language_code: "de",
                voice_id: "aura-2-viktoria-de",
                voice_label: "Viktoria (F)",
            },
            // French
            LanguageVoice {
                language_code: "fr",
                voice_id: "aura-2-agathe-fr",
                voice_label: "Agathe (F)",
            },
            LanguageVoice {
                language_code: "fr",
                voice_id: "aura-2-hector-fr",
                voice_label: "Hector (M)",
            },
            // Dutch
            LanguageVoice {
                language_code: "nl",
                voice_id: "aura-2-rhea-nl",
                voice_label: "Rhea (F)",
            },
            LanguageVoice {
                language_code: "nl",
                voice_id: "aura-2-sander-nl",
                voice_label: "Sander (M)",
            },
            // Italian
            LanguageVoice {
                language_code: "it",
                voice_id: "aura-2-livia-it",
                voice_label: "Livia (F)",
            },
            LanguageVoice {
                language_code: "it",
                voice_id: "aura-2-dionisio-it",
                voice_label: "Dionisio (M)",
            },
            // Japanese
            LanguageVoice {
                language_code: "ja",
                voice_id: "aura-2-fujin-ja",
                voice_label: "Fujin (M)",
            },
            LanguageVoice {
                language_code: "ja",
                voice_id: "aura-2-izanami-ja",
                voice_label: "Izanami (F)",
            },
        ],
    },
    // ── OpenAI TTS ───────────────────────────────────────────────────────────
    // OpenAI TTS uses named voices (alloy, echo, fable, onyx, nova, shimmer).
    // Voices are NOT language-specific — any named voice works with any language.
    // All three models support ~50 languages (follows Whisper language support).
    // gpt-4o-mini-tts additionally supports steerability (tone, accent, etc.)
    // Source: https://platform.openai.com/docs/guides/text-to-speech
    // gpt-4o-mini-tts: all 13 voices (including ballad, cedar, marin, verse added Oct 2024)
    TtsModelSpec {
        provider: "openai",
        model_id: "gpt-4o-mini-tts",
        label: "GPT-4o Mini TTS",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
        language_voices: &[
            LanguageVoice {
                language_code: "en",
                voice_id: "alloy",
                voice_label: "Alloy",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "ash",
                voice_label: "Ash",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "ballad",
                voice_label: "Ballad",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "cedar",
                voice_label: "Cedar",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "coral",
                voice_label: "Coral",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "echo",
                voice_label: "Echo",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "fable",
                voice_label: "Fable",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "marin",
                voice_label: "Marin",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "nova",
                voice_label: "Nova",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "onyx",
                voice_label: "Onyx",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "sage",
                voice_label: "Sage",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "shimmer",
                voice_label: "Shimmer",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "verse",
                voice_label: "Verse",
            },
        ],
    },
    // tts-1 / tts-1-hd: 9 voices (legacy set — no ballad, cedar, marin, verse)
    TtsModelSpec {
        provider: "openai",
        model_id: "tts-1",
        label: "TTS-1",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
        language_voices: &[
            LanguageVoice {
                language_code: "en",
                voice_id: "alloy",
                voice_label: "Alloy",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "ash",
                voice_label: "Ash",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "coral",
                voice_label: "Coral",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "echo",
                voice_label: "Echo",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "fable",
                voice_label: "Fable",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "nova",
                voice_label: "Nova",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "onyx",
                voice_label: "Onyx",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "sage",
                voice_label: "Sage",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "shimmer",
                voice_label: "Shimmer",
            },
        ],
    },
    TtsModelSpec {
        provider: "openai",
        model_id: "tts-1-hd",
        label: "TTS-1 HD",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
        language_voices: &[
            LanguageVoice {
                language_code: "en",
                voice_id: "alloy",
                voice_label: "Alloy",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "ash",
                voice_label: "Ash",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "coral",
                voice_label: "Coral",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "echo",
                voice_label: "Echo",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "fable",
                voice_label: "Fable",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "nova",
                voice_label: "Nova",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "onyx",
                voice_label: "Onyx",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "sage",
                voice_label: "Sage",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "shimmer",
                voice_label: "Shimmer",
            },
        ],
    },
    // ── Groq Orpheus TTS ──────────────────────────────────────────────────────
    // Groq hosts Orpheus TTS models by Canopy Labs.
    // orpheus-v1-english — 6 English voices with vocal direction support.
    // orpheus-arabic-saudi — 4 Arabic (Saudi dialect) voices.
    // Source: https://console.groq.com/docs/text-to-speech/orpheus
    TtsModelSpec {
        provider: "groq",
        model_id: "canopylabs/orpheus-v1-english",
        label: "Orpheus v1 English",
        supported_languages: &["en"],
        language_voices: &[
            LanguageVoice {
                language_code: "en",
                voice_id: "autumn",
                voice_label: "Autumn (F)",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "diana",
                voice_label: "Diana (F)",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "hannah",
                voice_label: "Hannah (F)",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "austin",
                voice_label: "Austin (M)",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "daniel",
                voice_label: "Daniel (M)",
            },
            LanguageVoice {
                language_code: "en",
                voice_id: "troy",
                voice_label: "Troy (M)",
            },
        ],
    },
    TtsModelSpec {
        provider: "groq",
        model_id: "canopylabs/orpheus-arabic-saudi",
        label: "Orpheus Arabic (Saudi)",
        supported_languages: &["ar"],
        language_voices: &[
            LanguageVoice {
                language_code: "ar",
                voice_id: "fahad",
                voice_label: "Fahad (M)",
            },
            LanguageVoice {
                language_code: "ar",
                voice_id: "sultan",
                voice_label: "Sultan (M)",
            },
            LanguageVoice {
                language_code: "ar",
                voice_id: "lulwa",
                voice_label: "Lulwa (F)",
            },
            LanguageVoice {
                language_code: "ar",
                voice_id: "noura",
                voice_label: "Noura (F)",
            },
        ],
    },
];

pub const STT_MODEL_CATALOG: &[SttModelSpec] = &[
    SttModelSpec {
        provider: "faster-whisper",
        model_id: "large-v3",
        label: "Whisper Large v3",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
    },
    SttModelSpec {
        provider: "faster-whisper",
        model_id: "large-v3-turbo",
        label: "Whisper Large v3 Turbo",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
    },
    SttModelSpec {
        provider: "faster-whisper",
        model_id: "medium",
        label: "Whisper Medium",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
    },
    SttModelSpec {
        provider: "deepgram",
        model_id: "nova-3-general",
        label: "Nova 3 General",
        supported_languages: &[
            "ar", "be", "bn", "bs", "bg", "ca", "zh", "hr", "cs", "da", "nl", "en", "et", "fi",
            "fr", "de", "el", "he", "hi", "hu", "id", "it", "ja", "kn", "ko", "lv", "lt", "mk",
            "ms", "mr", "no", "fa", "pl", "pt", "ro", "ru", "sr", "sk", "sl", "es", "sv", "tl",
            "ta", "te", "th", "tr", "uk", "ur", "vi",
        ],
    },
    SttModelSpec {
        provider: "deepgram",
        model_id: "nova-2-general",
        label: "Nova 2 General",
        supported_languages: &[
            "bg", "ca", "zh", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hi", "hu",
            "id", "it", "ja", "ko", "lv", "lt", "ms", "no", "pl", "pt", "ro", "ru", "sk", "es",
            "sv", "th", "tr", "uk", "vi",
        ],
    },
    SttModelSpec {
        provider: "deepgram",
        model_id: "nova-2-phonecall",
        label: "Nova 2 Phone-call",
        supported_languages: &["en"],
    },
    SttModelSpec {
        provider: "cartesia",
        model_id: "ink-whisper",
        label: "Ink Whisper",
        supported_languages: &[
            "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar",
            "sv", "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu",
            "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa",
            "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn",
            "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc",
            "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo", "ht", "ps", "tk", "nn",
            "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw",
            "su", "yue",
        ],
    },
    SttModelSpec {
        provider: "elevenlabs",
        model_id: "scribe_v1",
        label: "Scribe v1",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
    },
    SttModelSpec {
        provider: "elevenlabs",
        model_id: "scribe_v2",
        label: "Scribe v2",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
    },
    SttModelSpec {
        provider: "groq",
        model_id: "whisper-large-v3-turbo",
        label: "Whisper Large v3 Turbo",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
    },
    SttModelSpec {
        provider: "groq",
        model_id: "whisper-large-v3",
        label: "Whisper Large v3",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
    },
    SttModelSpec {
        provider: "openai-whisper",
        model_id: "whisper-1",
        label: "Whisper 1",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
    },
    SttModelSpec {
        provider: "openai-realtime",
        model_id: "gpt-4o-transcribe",
        label: "GPT-4o Transcribe",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
    },
    SttModelSpec {
        provider: "openai-realtime",
        model_id: "gpt-4o-mini-transcribe",
        label: "GPT-4o Mini Transcribe",
        supported_languages: &[
            "en", "es", "fr", "de", "pt", "it", "ja", "ko", "zh", "ar", "hi", "nl", "ru", "pl",
            "sv",
        ],
    },
];

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Strip a BCP-47 region suffix to obtain the ISO 639-1 base code.
///
/// Examples: `"en-US"` → `"en"`, `"pt-BR"` → `"pt"`, `"zh"` → `"zh"`
///
/// # Why this matters
/// If `"pt-BR"` were stored in the DB (valid BCP-47) and forwarded directly,
/// ElevenLabs would receive `language_code=pt-BR` which it silently rejects,
/// causing the TTS to fall back to its default language.
#[inline]
pub fn normalize_to_base_code(code: &str) -> &str {
    match code.find('-') {
        Some(idx) => &code[..idx],
        None => code,
    }
}

/// Return the ElevenLabs `language_code` URL parameter value for `iso_code`.
///
/// Returns `""` (empty string) if the code is unrecognised — callers treat
/// `""` as "don't send the parameter".
///
/// The caller is also responsible for the multilingual model gate:
/// only append `&language_code=…` when the configured model is in
/// `ELEVENLABS_MULTILINGUAL_MODELS`.
pub fn elevenlabs_language_code(iso_code: &str) -> &'static str {
    let base = normalize_to_base_code(iso_code);
    SUPPORTED_LANGUAGES
        .iter()
        .find(|l| l.code == base)
        .map(|l| l.elevenlabs_code)
        .unwrap_or("") // "" → caller omits &language_code param
}

/// Return the Cartesia `language` JSON field value for `iso_code`.
///
/// Returns `""` if unrecognised — callers omit the field, and Cartesia
/// defaults to English.
pub fn cartesia_language_code(iso_code: &str) -> &'static str {
    let base = normalize_to_base_code(iso_code);
    SUPPORTED_LANGUAGES
        .iter()
        .find(|l| l.code == base)
        .map(|l| l.cartesia_code)
        .unwrap_or("") // "" → caller omits "language" field
}

/// Find a model spec by provider and model_id.
///
/// Returns `None` if the (provider, model_id) pair is not in the catalog.
/// Callers should emit a warning when `None` is returned for a non-English
/// language (unknown model → no language guarantee).
pub fn tts_model_lookup(provider: &str, model_id: &str) -> Option<&'static TtsModelSpec> {
    TTS_MODEL_CATALOG
        .iter()
        .find(|m| m.provider == provider && m.model_id == model_id)
}

/// True if the given provider+model combination can synthesize `lang`.
///
/// English (`"en"`) and empty strings are always allowed as a safe fallback.
///
/// Models not found in `TTS_MODEL_CATALOG` return `false` for non-English
/// languages — this is intentional to flag misconfiguration. Contributors
/// should add new models to the catalog to lift this restriction.
pub fn tts_model_supports_language(provider: &str, model_id: &str, lang: &str) -> bool {
    if lang.is_empty() || lang == "en" {
        return true;
    }
    let base = normalize_to_base_code(lang);
    TTS_MODEL_CATALOG
        .iter()
        .find(|m| m.provider == provider && m.model_id == model_id)
        .map(|m| m.supported_languages.contains(&base))
        .unwrap_or(false) // unknown model → conservative false
}

/// Return `true` if the given STT model supports `lang`.
pub fn stt_model_supports_language(provider: &str, model_id: &str, lang: &str) -> bool {
    if lang.is_empty() || lang == "en" {
        return true;
    }
    let base = normalize_to_base_code(lang);
    STT_MODEL_CATALOG
        .iter()
        .find(|m| m.provider == provider && m.model_id == model_id)
        .map(|m| m.supported_languages.contains(&base))
        .unwrap_or(false)
}

/// Return catalog entries (for any provider) that support the given language.
///
/// Used by the backend to suggest compatible models when the agent's current
/// model doesn't support the newly selected language.
pub fn tts_models_for_language(lang: &str) -> impl Iterator<Item = &'static TtsModelSpec> {
    let base: &'static str = {
        // We need a 'static str for the closure. Find the matching SUPPORTED_LANGUAGES code.
        let base_slice = normalize_to_base_code(lang);
        SUPPORTED_LANGUAGES
            .iter()
            .find(|l| l.code == base_slice)
            .map(|l| l.code)
            .unwrap_or("")
    };
    TTS_MODEL_CATALOG
        .iter()
        .filter(move |m| m.supported_languages.contains(&base))
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_region() {
        assert_eq!(normalize_to_base_code("en-US"), "en");
        assert_eq!(normalize_to_base_code("pt-BR"), "pt");
        assert_eq!(normalize_to_base_code("zh-TW"), "zh");
        assert_eq!(normalize_to_base_code("en"), "en");
        assert_eq!(normalize_to_base_code(""), "");
    }

    #[test]
    fn elevenlabs_lookup_with_region_code() {
        assert_eq!(elevenlabs_language_code("es-ES"), "es");
        assert_eq!(elevenlabs_language_code("pt-BR"), "pt");
    }

    #[test]
    fn unsupported_code_returns_empty() {
        assert_eq!(elevenlabs_language_code("xx"), "");
        assert_eq!(cartesia_language_code("xx-XX"), "");
    }

    #[test]
    fn all_supported_languages_have_consistent_codes() {
        for lang in SUPPORTED_LANGUAGES {
            assert!(!lang.code.is_empty(), "code empty for {}", lang.label);
            assert!(!lang.label.is_empty(), "label empty for {}", lang.code);
            assert!(
                !lang.elevenlabs_code.is_empty(),
                "elevenlabs_code empty for {}",
                lang.code
            );
            assert!(
                !lang.deepgram_code.is_empty(),
                "deepgram_code empty for {}",
                lang.code
            );
            assert!(
                !lang.cartesia_code.is_empty(),
                "cartesia_code empty for {}",
                lang.code
            );
        }
    }

    #[test]
    fn elevenlabs_multilingual_models_not_empty() {
        assert!(!ELEVENLABS_MULTILINGUAL_MODELS.is_empty());
    }

    // ── TTS catalog integrity ────────────────────────────────────────────────

    #[test]
    fn catalog_language_codes_are_valid() {
        let valid_codes: std::collections::HashSet<&str> =
            SUPPORTED_LANGUAGES.iter().map(|l| l.code).collect();
        for spec in TTS_MODEL_CATALOG {
            for &lang in spec.supported_languages {
                assert!(
                    valid_codes.contains(lang),
                    "TTS_MODEL_CATALOG[{}/{}].supported_languages contains '{}' \
                     which is not in SUPPORTED_LANGUAGES",
                    spec.provider,
                    spec.model_id,
                    lang,
                );
            }
            for voice in spec.language_voices {
                assert!(
                    valid_codes.contains(voice.language_code),
                    "TTS_MODEL_CATALOG[{}/{}].language_voices contains language_code '{}' \
                     which is not in SUPPORTED_LANGUAGES",
                    spec.provider,
                    spec.model_id,
                    voice.language_code,
                );
                assert!(
                    !voice.voice_id.is_empty(),
                    "TTS_MODEL_CATALOG[{}/{}] voice '{}' has empty voice_id",
                    spec.provider,
                    spec.model_id,
                    voice.voice_label,
                );
            }
        }
    }

    #[test]
    fn catalog_every_model_has_at_least_one_voice() {
        for spec in TTS_MODEL_CATALOG {
            assert!(
                !spec.language_voices.is_empty(),
                "TTS_MODEL_CATALOG[{}/{}] has no language_voices entries",
                spec.provider,
                spec.model_id,
            );
        }
    }

    #[test]
    fn tts_model_supports_language_multilingual() {
        // Cartesia sonic-2 supports Japanese
        assert!(tts_model_supports_language("cartesia-ws", "sonic-2", "ja"));
        // ElevenLabs flash supports Chinese
        assert!(tts_model_supports_language(
            "elevenlabs-ws",
            "eleven_flash_v2_5",
            "zh"
        ));
        // Deepgram Aura supports English and Japanese
        assert!(tts_model_supports_language("deepgram-ws", "aura-2", "en"));
        assert!(tts_model_supports_language("deepgram-ws", "aura-2", "ja"));
        // Deepgram does not support Chinese
        assert!(!tts_model_supports_language("deepgram-ws", "aura-2", "zh"));
    }

    #[test]
    fn tts_model_supports_language_english_always_ok() {
        // English is always allowed — even for unknown models
        assert!(tts_model_supports_language(
            "unknown-provider",
            "unknown-model",
            "en"
        ));
        assert!(tts_model_supports_language(
            "unknown-provider",
            "unknown-model",
            ""
        ));
    }

    #[test]
    fn tts_model_supports_language_unknown_model_returns_false() {
        // sonic-english not in catalog → false for non-English
        assert!(!tts_model_supports_language(
            "cartesia-ws",
            "sonic-english",
            "es"
        ));
    }

    #[test]
    fn tts_models_for_language_returns_multilingual_models() {
        let zh_models: Vec<_> = tts_models_for_language("zh").collect();
        // Cartesia + ElevenLabs (WS + HTTP variants) should all appear
        assert!(zh_models.iter().any(|m| m.model_id == "sonic-2"));
        assert!(zh_models.iter().any(|m| m.model_id == "eleven_flash_v2_5"));
        // Deepgram should NOT appear
        assert!(!zh_models.iter().any(|m| m.provider.starts_with("deepgram")));
    }

    #[test]
    fn default_voice_for_language_multilingual() {
        let spec = tts_model_lookup("cartesia-ws", "sonic-2").unwrap();
        // Any language falls back to the single English default for multilingual models
        assert_eq!(
            spec.default_voice_for_language("ja"),
            Some("694f9389-aac1-45b6-b726-9d9369183238")
        );
        assert_eq!(
            spec.default_voice_for_language("en"),
            Some("694f9389-aac1-45b6-b726-9d9369183238")
        );
    }

    #[test]
    fn default_voice_for_language_deepgram() {
        let spec = tts_model_lookup("deepgram-ws", "aura-2").unwrap();
        // English → Thalia (first English entry)
        assert_eq!(
            spec.default_voice_for_language("en"),
            Some("aura-2-thalia-en")
        );
    }
}
