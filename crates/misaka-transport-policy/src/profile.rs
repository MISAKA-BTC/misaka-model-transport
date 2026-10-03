//! The file classes of the Safe Model Profile v1 (§5.1) and the kind constraints of §2.1.
//!
//! The profile is an allowlist. A name is classified by its exact spelling; anything not listed
//! is refused. Formats that execute on load, code, binaries and archives are refused by name, so
//! that a reader does not have to infer it.

use misaka_bundle::{BundleKind, DESCRIPTOR_FILE_NAME, PalwMagic, Role};

use crate::Refusal;
use crate::limits::MIB;

/// What a file is, by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileClass {
    /// `*.palwart`, `*.palwq36`, `*.palwtir`: the magics the extension allows.
    PalwContainer(&'static [PalwMagic]),
    /// `*.palwmanifest`: ≤ 16 MiB, UTF-8 JSON.
    PalwManifest,
    /// `*.gguf`.
    Gguf,
    /// `*.safetensors`.
    Safetensors,
    /// `misaka-bundle.json`.
    Descriptor,
    /// A JSON metadata file (≤ 64 MiB, UTF-8 JSON) with its role.
    JsonMetadata(Role),
    /// `tokenizer.model`, `*.tiktoken`, `merges.txt`: ≤ 64 MiB, not inspected.
    TokenizerData,
    /// `LICENSE`, `LICENSE.txt`, `NOTICE`, `README.md`: ≤ 1 MiB, UTF-8.
    Text(Role),
}

/// What L2 checks in a file's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentCheck {
    /// The first 8 bytes are one of the allowed PALW magics and equal the descriptor's.
    PalwMagic,
    /// The whole file is UTF-8 JSON.
    Json,
    /// The first 4 bytes are `GGUF`.
    Gguf,
    /// An 8-byte little-endian length ≤ 100 MiB, then that many bytes of JSON.
    Safetensors,
    /// The whole file is UTF-8.
    Utf8,
    /// Nothing beyond the size.
    None,
}

const PALWART: &[PalwMagic] = &[PalwMagic::PalwB0A2, PalwMagic::PalwB0A1];
const PALWQ36: &[PalwMagic] = &[PalwMagic::PalwQ361];
const PALWTIR: &[PalwMagic] = &[PalwMagic::PalwTir1];

/// Refused by name (§5.1): extension (lowercased) → why.
const REFUSED_EXTENSIONS: &[(&str, &str)] = &[
    ("bin", "the pickle family (loading a pickle can run code)"),
    ("pt", "the pickle family (loading a pickle can run code)"),
    ("pth", "the pickle family (loading a pickle can run code)"),
    ("ckpt", "the pickle family (loading a pickle can run code)"),
    ("pkl", "the pickle family (loading a pickle can run code)"),
    ("pickle", "the pickle family (loading a pickle can run code)"),
    ("joblib", "the pickle family (loading a pickle can run code)"),
    ("npy", "the pickle family (numpy object arrays unpickle)"),
    ("npz", "the pickle family (numpy object arrays unpickle)"),
    ("onnx", "a format that can carry custom operators"),
    ("h5", "a format that executes on load (lambda layers)"),
    ("keras", "a format that executes on load (lambda layers)"),
    ("py", "code"),
    ("pyc", "code"),
    ("ipynb", "code"),
    ("js", "code"),
    ("sh", "code"),
    ("bat", "code"),
    ("cmd", "code"),
    ("ps1", "code"),
    ("exe", "a binary"),
    ("dll", "a binary"),
    ("so", "a binary"),
    ("dylib", "a binary"),
    ("zip", "an archive"),
    ("tar", "an archive"),
    ("gz", "an archive"),
    ("tgz", "an archive"),
    ("7z", "an archive"),
    ("rar", "an archive"),
    ("zst", "an archive"),
];

/// Classifies a name, or refuses it.
pub fn classify(name: &str) -> Result<FileClass, Refusal> {
    let exact = match name {
        DESCRIPTOR_FILE_NAME => Some(FileClass::Descriptor),
        "config.json" | "generation_config.json" => Some(FileClass::JsonMetadata(Role::Config)),
        "model.safetensors.index.json" => Some(FileClass::JsonMetadata(Role::WeightsIndex)),
        "tokenizer.json" | "tokenizer_config.json" | "special_tokens_map.json" | "vocab.json" => {
            Some(FileClass::JsonMetadata(Role::Tokenizer))
        }
        "tokenizer.model" | "merges.txt" => Some(FileClass::TokenizerData),
        "LICENSE" | "LICENSE.txt" => Some(FileClass::Text(Role::License)),
        "NOTICE" => Some(FileClass::Text(Role::Notice)),
        "README.md" => Some(FileClass::Text(Role::Readme)),
        _ => None,
    };
    if let Some(c) = exact {
        return Ok(c);
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let by_ext = match ext {
        "palwart" => Some(FileClass::PalwContainer(PALWART)),
        "palwq36" => Some(FileClass::PalwContainer(PALWQ36)),
        "palwtir" => Some(FileClass::PalwContainer(PALWTIR)),
        "palwmanifest" => Some(FileClass::PalwManifest),
        "gguf" => Some(FileClass::Gguf),
        "safetensors" => Some(FileClass::Safetensors),
        "tiktoken" => Some(FileClass::TokenizerData),
        _ => None,
    };
    if let Some(c) = by_ext {
        return Ok(c);
    }
    let lower = ext.to_ascii_lowercase();
    if let Some((_, why)) = REFUSED_EXTENSIONS.iter().find(|(e, _)| *e == lower) {
        return Err(Refusal::RefusedFormat { name: name.to_owned(), why });
    }
    Err(Refusal::NotAllowed(name.to_owned()))
}

impl FileClass {
    /// The role a descriptor entry with this class carries; `None` for the descriptor itself.
    pub fn role(self) -> Option<Role> {
        Some(match self {
            FileClass::PalwContainer(_) => Role::PalwContainer,
            FileClass::PalwManifest => Role::PalwManifest,
            FileClass::Gguf | FileClass::Safetensors => Role::Weights,
            FileClass::Descriptor => return None,
            FileClass::JsonMetadata(r) | FileClass::Text(r) => r,
            FileClass::TokenizerData => Role::Tokenizer,
        })
    }

    /// The class's own size bound, below the bundle's 4 TiB.
    pub fn max_size(self) -> Option<u64> {
        match self {
            FileClass::PalwManifest => Some(16 * MIB),
            FileClass::Descriptor => Some(misaka_bundle::MAX_DESCRIPTOR_BYTES as u64),
            FileClass::JsonMetadata(_) | FileClass::TokenizerData => Some(64 * MIB),
            FileClass::Text(_) => Some(MIB),
            FileClass::PalwContainer(_) | FileClass::Gguf | FileClass::Safetensors => None,
        }
    }

    pub fn content_check(self) -> ContentCheck {
        match self {
            FileClass::PalwContainer(_) => ContentCheck::PalwMagic,
            FileClass::PalwManifest | FileClass::Descriptor | FileClass::JsonMetadata(_) => ContentCheck::Json,
            FileClass::Gguf => ContentCheck::Gguf,
            FileClass::Safetensors => ContentCheck::Safetensors,
            FileClass::Text(_) => ContentCheck::Utf8,
            FileClass::TokenizerData => ContentCheck::None,
        }
    }
}

/// The kind a file set implies: a PALW container makes it `palw-artifact`.
pub fn infer_kind<'a>(classes: impl IntoIterator<Item = &'a FileClass>) -> BundleKind {
    if classes.into_iter().any(|c| matches!(c, FileClass::PalwContainer(_))) {
        BundleKind::PalwArtifact
    } else {
        BundleKind::SourceWeights
    }
}

/// The constraints of §2.1 on a bundle's file set, the descriptor included.
pub fn check_kind(kind: BundleKind, files: &[(&str, FileClass)]) -> Result<(), Refusal> {
    let refuse = |why: String| Err(Refusal::Kind { kind, why });
    let count = |pred: &dyn Fn(&FileClass) -> bool| files.iter().filter(|(_, c)| pred(c)).count();
    if count(&|c| *c == FileClass::Descriptor) != 1 {
        return Err(Refusal::NoDescriptor);
    }
    match kind {
        BundleKind::PalwArtifact => {
            let containers = count(&|c| matches!(c, FileClass::PalwContainer(_)));
            if containers != 1 {
                return refuse(format!("holds exactly one PALW container, found {containers}"));
            }
            if count(&|c| *c == FileClass::PalwManifest) > 1 {
                return refuse("holds at most one .palwmanifest".into());
            }
            for (name, c) in files {
                let ok = matches!(
                    c,
                    FileClass::PalwContainer(_) | FileClass::PalwManifest | FileClass::Text(_) | FileClass::Descriptor
                );
                if !ok {
                    return refuse(format!("{name:?} belongs in a source-weights bundle"));
                }
            }
        }
        BundleKind::SourceWeights => {
            for (name, c) in files {
                if matches!(c, FileClass::PalwContainer(_) | FileClass::PalwManifest) {
                    return refuse(format!("{name:?} belongs in a palw-artifact bundle"));
                }
            }
            let gguf = count(&|c| *c == FileClass::Gguf);
            let st = count(&|c| *c == FileClass::Safetensors);
            let index = count(&|c| *c == FileClass::JsonMetadata(Role::WeightsIndex));
            match (gguf, st) {
                (1, 0) if index == 0 => {}
                (1, 0) => return refuse("model.safetensors.index.json without safetensors shards".into()),
                (0, 1) => {}
                (0, n) if n > 1 && index == 1 => {}
                (0, n) if n > 1 => return refuse("safetensors shards need model.safetensors.index.json".into()),
                (0, 0) => return refuse("holds no weights (one .gguf, or .safetensors shards)".into()),
                _ => return refuse("holds one .gguf, or .safetensors shards, not both or several .gguf".into()),
            }
        }
        BundleKind::Adapter | BundleKind::Shard => return Err(Refusal::ReservedKind(kind)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies() {
        assert_eq!(classify("qwen.palwart").unwrap(), FileClass::PalwContainer(PALWART));
        assert_eq!(classify("model-00001-of-00002.safetensors").unwrap(), FileClass::Safetensors);
        assert_eq!(classify("LICENSE").unwrap().role(), Some(Role::License));
        assert_eq!(classify("tokenizer_config.json").unwrap().role(), Some(Role::Tokenizer));
        assert_eq!(classify("cl100k.tiktoken").unwrap(), FileClass::TokenizerData);
        for (bad, why) in [
            ("pytorch_model.bin", "pickle"),
            ("model.PT", "pickle"),
            ("weights.npz", "pickle"),
            ("modeling_qwen.py", "code"),
            ("model.onnx", "custom operators"),
            ("x.tar", "archive"),
            ("lib.dylib", "binary"),
        ] {
            match classify(bad) {
                Err(Refusal::RefusedFormat { why: w, .. }) => assert!(w.contains(why), "{bad}: {w}"),
                other => panic!("{bad}: {other:?}"),
            }
        }
        assert!(matches!(classify("notes.txt"), Err(Refusal::NotAllowed(_))));
        assert!(matches!(classify("model.GGUF"), Err(Refusal::NotAllowed(_))));
        assert!(matches!(classify("other.json"), Err(Refusal::NotAllowed(_))));
    }

    fn set(names: &[&'static str]) -> Vec<(&'static str, FileClass)> {
        names.iter().map(|n| (*n, classify(n).unwrap())).collect()
    }

    #[test]
    fn kinds() {
        let pa = BundleKind::PalwArtifact;
        let sw = BundleKind::SourceWeights;
        assert!(
            check_kind(pa, &set(&["misaka-bundle.json", "a.palwart", "a.palwmanifest", "LICENSE", "README.md"]))
                .is_ok()
        );
        assert!(check_kind(pa, &set(&["misaka-bundle.json", "a.palwart", "b.palwq36", "LICENSE"])).is_err());
        assert!(check_kind(pa, &set(&["misaka-bundle.json", "a.palwart", "config.json", "LICENSE"])).is_err());
        assert!(matches!(check_kind(pa, &set(&["a.palwart", "LICENSE"])), Err(Refusal::NoDescriptor)));
        assert!(check_kind(sw, &set(&["misaka-bundle.json", "m.gguf", "LICENSE"])).is_ok());
        assert!(
            check_kind(
                sw,
                &set(&["misaka-bundle.json", "model.safetensors", "config.json", "tokenizer.json", "LICENSE"])
            )
            .is_ok()
        );
        assert!(
            check_kind(
                sw,
                &set(&[
                    "misaka-bundle.json",
                    "a.safetensors",
                    "b.safetensors",
                    "model.safetensors.index.json",
                    "LICENSE"
                ])
            )
            .is_ok()
        );
        assert!(check_kind(sw, &set(&["misaka-bundle.json", "a.safetensors", "b.safetensors", "LICENSE"])).is_err());
        assert!(check_kind(sw, &set(&["misaka-bundle.json", "a.gguf", "b.gguf", "LICENSE"])).is_err());
        assert!(check_kind(sw, &set(&["misaka-bundle.json", "a.gguf", "b.safetensors", "LICENSE"])).is_err());
        assert!(check_kind(sw, &set(&["misaka-bundle.json", "LICENSE"])).is_err());
        assert!(check_kind(sw, &set(&["misaka-bundle.json", "m.gguf", "a.palwmanifest", "LICENSE"])).is_err());
    }
}
