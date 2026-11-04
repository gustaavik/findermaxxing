//! The pinned model files: repository, revision, size and SHA-256 of every
//! file, so a download is exactly what was tested and nothing else.

use application::ports::ModelPack;
use domain::ByteSize;

pub(crate) const HUGGING_FACE: &str = "https://huggingface.co";

#[derive(Debug, Clone, Copy)]
pub(crate) struct ModelFile {
    pub repo: &'static str,
    pub revision: &'static str,
    pub path: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// Where it is kept below the pack's folder.
    pub local: &'static str,
}

const E5: &str = "intfloat/multilingual-e5-small";
const E5_REVISION: &str = "614241f622f53c4eeff9890bdc4f31cfecc418b3";
const MCLIP: &str = "sentence-transformers/clip-ViT-B-32-multilingual-v1";
const MCLIP_REVISION: &str = "58edf8cada9e398793dca955574a48cbb7f18be2";
const CLIP: &str = "openai/clip-vit-base-patch32";
const CLIP_REVISION: &str = "3d74acf9a28c67741b2f4f2ea7635f0aaf6f0268";

pub(crate) const TEXT_FILES: &[ModelFile] = &[
    ModelFile {
        repo: E5,
        revision: E5_REVISION,
        path: "config.json",
        size: 655,
        sha256: "69137736cab8b8903a07fe8afaafdda25aac55415a12a55d1bffa9f581abf959",
        local: "e5/config.json",
    },
    ModelFile {
        repo: E5,
        revision: E5_REVISION,
        path: "tokenizer.json",
        size: 17_082_730,
        sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
        local: "e5/tokenizer.json",
    },
    ModelFile {
        repo: E5,
        revision: E5_REVISION,
        path: "model.safetensors",
        size: 470_641_600,
        sha256: "1a55775f53449dac10a2bcbc312469fac40b96d53198c407081a831f81c98477",
        local: "e5/model.safetensors",
    },
];

pub(crate) const IMAGE_FILES: &[ModelFile] = &[
    ModelFile {
        repo: MCLIP,
        revision: MCLIP_REVISION,
        path: "config.json",
        size: 572,
        sha256: "edc12f518bfc445dedfe8a7c58374ea659525e0b6e67814686bd6cd57f90f8b3",
        local: "mclip/config.json",
    },
    ModelFile {
        repo: MCLIP,
        revision: MCLIP_REVISION,
        path: "tokenizer.json",
        size: 1_961_847,
        sha256: "5b4e1a8171c81dfd666ae40265b9530c6e0b3d53923fe8ac493dcc84229adf81",
        local: "mclip/tokenizer.json",
    },
    ModelFile {
        repo: MCLIP,
        revision: MCLIP_REVISION,
        path: "model.safetensors",
        size: 538_947_416,
        sha256: "7b968c09938603158413ea2c91af82e82158ebb6db901a6cf97c28ce7d7ebc6d",
        local: "mclip/model.safetensors",
    },
    ModelFile {
        repo: MCLIP,
        revision: MCLIP_REVISION,
        path: "2_Dense/model.safetensors",
        size: 1_572_984,
        sha256: "d12568dc7300970a4d3dbb49068ad16cd89b99840b74b026f8e48071e9414f74",
        local: "mclip/dense.safetensors",
    },
    ModelFile {
        repo: CLIP,
        revision: CLIP_REVISION,
        path: "pytorch_model.bin",
        size: 605_247_071,
        sha256: "a63082132ba4f97a80bea76823f544493bffa8082296d62d71581a4feff1576f",
        local: "clip/pytorch_model.bin",
    },
];

pub(crate) fn files(pack: ModelPack) -> &'static [ModelFile] {
    match pack {
        ModelPack::Text => TEXT_FILES,
        ModelPack::Images => IMAGE_FILES,
    }
}

pub(crate) fn download_size(pack: ModelPack) -> ByteSize {
    ByteSize(files(pack).iter().map(|f| f.size).sum())
}

pub(crate) fn pack_folder(pack: ModelPack) -> &'static str {
    match pack {
        ModelPack::Text => "text",
        ModelPack::Images => "images",
    }
}

/// Identifies the embedding space; stored with every indexed item.
pub(crate) fn model_id(pack: ModelPack) -> String {
    match pack {
        ModelPack::Text => format!("{E5}@{}", &E5_REVISION[..12]),
        ModelPack::Images => format!("{MCLIP}+{CLIP}@{}", &CLIP_REVISION[..12]),
    }
}

pub(crate) fn source(pack: ModelPack) -> String {
    let repos: Vec<&str> =
        files(pack).iter().map(|f| f.repo).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    format!("{HUGGING_FACE} ({})", repos.join(", "))
}
