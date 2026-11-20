//! The speech models offered in Settings. Every file is a Q8_0 GGUF published
//! by handy-computer for transcribe.cpp, pinned to one Hugging Face revision and
//! verified by its exact size and SHA-256 after downloading.

/// A speech model offered in Settings.
#[derive(Debug, PartialEq, Eq)]
pub struct SpeechModel {
    /// The value saved in `TranscriptionSettings::model`.
    pub id: &'static str,
    /// Identifies the engine and model that produced a stored transcript.
    pub name: &'static str,
    /// Shown in Settings and transcript tooltips (a product name, not translated).
    pub title: &'static str,
    /// Its role among the choices, in English; the UI translates it.
    pub role: &'static str,
    /// What it is good at, in English; the UI translates it.
    pub detail: &'static str,
    /// How many languages it transcribes.
    pub languages: usize,
    /// Credit the model's license asks for, shown in Settings.
    pub attribution: Option<&'static str>,
    repository: &'static str,
    revision: &'static str,
    pub(super) file: &'static str,
    sha256: &'static str,
    pub bytes: u64,
    /// The GGUF's `general.architecture` and variant, checked after loading.
    pub(super) architecture: &'static str,
    pub(super) variant: &'static str,
    /// Earlier ids and transcript names for this model (whisper.cpp builds).
    legacy_ids: &'static [&'static str],
    legacy_names: &'static [&'static str],
}

/// Whisper's 99 languages; Parakeet TDT v3's 25 European ones are listed in
/// [`PARAKEET_V3_LANGUAGES`].
const WHISPER_LANGUAGES: usize = 99;
/// The languages Parakeet TDT 0.6B v3 transcribes. It detects which one it
/// hears and takes no language hint.
pub const PARAKEET_V3_LANGUAGES: [&str; 25] = [
    "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv", "lt", "mt",
    "pl", "pt", "ro", "ru", "sk", "sl", "es", "sv", "uk",
];

pub const MODELS: [SpeechModel; 4] = [
    SpeechModel {
        id: "whisper-base",
        name: "whisper-base-q8_0",
        title: "Whisper base",
        role: "Light",
        detail: "Quick on any computer; often misses words, especially outside English.",
        languages: WHISPER_LANGUAGES,
        attribution: None,
        repository: "handy-computer/whisper-base-gguf",
        revision: "42fcd2077de47c36d779309e0e91b76ec620f56d",
        file: "whisper-base-Q8_0.gguf",
        sha256: "81c069428bc8a24551a8169cf31cf09bcfd9d4cf50389ae281323c9aa9648c81",
        bytes: 84_962_880,
        architecture: "whisper",
        variant: "whisper-base",
        legacy_ids: &["base"],
        legacy_names: &["whisper-base-q5_1"],
    },
    SpeechModel {
        id: "whisper-small",
        name: "whisper-small-q8_0",
        title: "Whisper small",
        role: "Balanced",
        detail: "Much more accurate than Whisper base; about three times slower.",
        languages: WHISPER_LANGUAGES,
        attribution: None,
        repository: "handy-computer/whisper-small-gguf",
        revision: "4afdc7942ecf1997f359deae48a63506b062eac4",
        file: "whisper-small-Q8_0.gguf",
        sha256: "9b9c8811bbcc82a7766f0fb0925614bdacb0923b2cc630daeac17108b655b860",
        bytes: 269_751_136,
        architecture: "whisper",
        variant: "whisper-small",
        legacy_ids: &["small"],
        legacy_names: &["whisper-small-q5_1"],
    },
    SpeechModel {
        id: "parakeet-v3",
        name: "parakeet-tdt-0.6b-v3-q8_0",
        title: "Parakeet v3",
        role: "Best for European languages",
        detail: "Fast and accurate in 25 European languages, which it detects from the audio. Other languages come out wrong.",
        languages: PARAKEET_V3_LANGUAGES.len(),
        attribution: Some(
            "Parakeet TDT 0.6B v3 by NVIDIA, licensed under CC BY 4.0; converted to GGUF by handy-computer.",
        ),
        repository: "handy-computer/parakeet-tdt-0.6b-v3-gguf",
        revision: "85ac09ea12fc4b1112fa76810059364bc6adc9de",
        file: "parakeet-tdt-0.6b-v3-Q8_0.gguf",
        sha256: "5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7",
        bytes: 739_508_576,
        architecture: "parakeet",
        variant: "tdt-0.6b-v3",
        legacy_ids: &[],
        legacy_names: &[],
    },
    SpeechModel {
        id: "whisper-large-v3-turbo",
        name: "whisper-large-v3-turbo-q8_0",
        title: "Whisper large-v3 turbo",
        role: "Most accurate",
        detail: "Best quality in 99 languages, especially for names; the slowest and needs about 1 GB of memory while transcribing.",
        languages: WHISPER_LANGUAGES,
        attribution: None,
        repository: "handy-computer/whisper-large-v3-turbo-gguf",
        revision: "d222c9f621c1128299248f2ded4d8a1820519780",
        file: "whisper-large-v3-turbo-Q8_0.gguf",
        sha256: "d5e65f2b0828802ae2c231673d31982cebe3a778c95d9494a9f3efee6bd17448",
        bytes: 886_381_824,
        architecture: "whisper",
        variant: "whisper-large-v3-turbo",
        legacy_ids: &["large-v3-turbo"],
        legacy_names: &["whisper-large-v3-turbo-q5_0"],
    },
];
pub const DEFAULT_MODEL: &str = "whisper-base";

/// whisper.cpp model files earlier versions downloaded into the same folder.
/// They are removed once a model of this catalog is ready.
pub(super) const LEGACY_FILES: [&str; 3] = [
    "ggml-base-q5_1.bin",
    "ggml-small-q5_1.bin",
    "ggml-large-v3-turbo-q5_0.bin",
];

impl SpeechModel {
    /// The model saved in settings; unknown values fall back to the default.
    pub fn find(id: &str) -> &'static SpeechModel {
        Self::get(id).unwrap_or(&MODELS[0])
    }
    /// A catalog id, or one saved by an earlier version.
    pub fn get(id: &str) -> Option<&'static SpeechModel> {
        MODELS
            .iter()
            .find(|m| m.id == id || m.legacy_ids.contains(&id))
    }
    /// A stored transcript's `model` column, for display.
    pub fn by_name(name: &str) -> Option<&'static SpeechModel> {
        MODELS
            .iter()
            .find(|m| m.name == name || m.legacy_names.contains(&name))
    }
    /// The id to save for `id`, which may come from an earlier version.
    pub fn migrate(id: &str) -> String {
        Self::get(id).map_or(id, |m| m.id).to_owned()
    }
    pub(super) fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repository, self.revision, self.file
        )
    }
    pub(super) fn sha256(&self) -> &'static str {
        self.sha256
    }
    pub(super) fn path(&self, cache: &std::path::Path) -> std::path::PathBuf {
        super::files::folder(cache).join(self.file)
    }
    pub fn downloaded(&self, cache: &std::path::Path) -> bool {
        std::fs::metadata(self.path(cache)).map(|m| m.len()).ok() == Some(self.bytes)
    }
    /// Whisper takes the app language as a hint; Parakeet always detects it.
    #[cfg_attr(not(feature = "transcription"), allow(dead_code))]
    pub(super) fn takes_language_hint(&self) -> bool {
        self.architecture == "whisper"
    }
}
