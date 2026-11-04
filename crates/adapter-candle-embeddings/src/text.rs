//! multilingual-e5-small: BERT, mean pooling, L2 normalisation, with the
//! `query: ` / `passage: ` prefixes the model was trained with.

use std::path::Path;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

use crate::{EmbedResult, failed, mean_pool, normalise};

pub(crate) const QUERY_PREFIX: &str = "query: ";
pub(crate) const PASSAGE_PREFIX: &str = "passage: ";
const MAX_TOKENS: usize = 512;

pub(crate) struct E5 {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl E5 {
    pub(crate) fn load(dir: &Path) -> EmbedResult<Self> {
        let config: Config =
            serde_json::from_str(&std::fs::read_to_string(dir.join("e5/config.json")).map_err(|e| failed(&e))?)
                .map_err(|e| failed(&e))?;
        let tokenizer = tokenizer(&dir.join("e5/tokenizer.json"), MAX_TOKENS)?;
        let device = Device::Cpu;
        // SAFETY: the file is ours, verified at install, and not changed while mapped.
        let weights = unsafe {
            VarBuilder::from_mmaped_safetensors(&[dir.join(crate::prepare::E5_WEIGHTS)], DType::F32, &device)
        }
        .map_err(|e| failed(&e))?;
        let model = BertModel::load(weights, &config).map_err(|e| failed(&e))?;
        Ok(Self { model, tokenizer, device })
    }

    pub(crate) fn embed(&self, texts: &[String]) -> EmbedResult<Vec<Vec<f32>>> {
        let encodings = self.tokenizer.encode_batch(texts.to_vec(), true).map_err(|e| failed(&e))?;
        let ids = stack(&encodings, |e| e.get_ids(), &self.device)?;
        let mask = stack(&encodings, |e| e.get_attention_mask(), &self.device)?;
        let types = ids.zeros_like().map_err(|e| failed(&e))?;
        let hidden = self.model.forward(&ids, &types, Some(&mask)).map_err(|e| failed(&e))?;
        normalise(&mean_pool(&hidden, &mask)?)
    }
}

pub(crate) fn tokenizer(path: &Path, max_tokens: usize) -> EmbedResult<Tokenizer> {
    let mut tokenizer = Tokenizer::from_file(path).map_err(|e| failed(&e))?;
    tokenizer
        .with_truncation(Some(TruncationParams { max_length: max_tokens, ..TruncationParams::default() }))
        .map_err(|e| failed(&e))?;
    tokenizer.with_padding(Some(PaddingParams::default()));
    Ok(tokenizer)
}

/// One row per encoding, as a `(batch, tokens)` u32 tensor.
pub(crate) fn stack(
    encodings: &[tokenizers::Encoding],
    field: impl Fn(&tokenizers::Encoding) -> &[u32],
    device: &Device,
) -> EmbedResult<Tensor> {
    let rows: Vec<Tensor> =
        encodings.iter().map(|e| Tensor::new(field(e), device)).collect::<Result<_, _>>().map_err(|e| failed(&e))?;
    Tensor::stack(&rows, 0).map_err(|e| failed(&e))
}
