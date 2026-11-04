//! Multilingual CLIP: images through CLIP ViT-B/32's vision tower and
//! projection; text through the multilingual DistilBERT that was trained
//! to land in the same 512-dimensional space (mean pooling, then a
//! 768→512 linear layer). Both normalised.

use std::path::Path;

use candle_core::{DType, Device, Module, Tensor};
use candle_nn::{Linear, VarBuilder};
use candle_transformers::models::clip::vision_model::{ClipVisionConfig, ClipVisionTransformer};
use candle_transformers::models::distilbert::{Config as DistilBertConfig, DistilBertModel};
use tokenizers::Tokenizer;

use crate::text::{stack, tokenizer};
use crate::{EmbedResult, failed, mean_pool, normalise};

const MAX_TOKENS: usize = 128;
const IMAGE_SIZE: u32 = 224;
const CLIP_DIMENSIONS: usize = 512;
const VISION_HIDDEN: usize = 768;
const MCLIP_HIDDEN: usize = 768;
/// CLIP's preprocessing (`preprocessor_config.json`).
const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
const STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];

pub(crate) struct ClipText {
    model: DistilBertModel,
    dense: Linear,
    tokenizer: Tokenizer,
    device: Device,
}

pub(crate) struct ClipVision {
    vision: ClipVisionTransformer,
    projection: Linear,
    device: Device,
}

fn weights(path: &Path, device: &Device) -> EmbedResult<VarBuilder<'static>> {
    // SAFETY: the file is ours, verified at install, and not changed while mapped.
    unsafe { VarBuilder::from_mmaped_safetensors(&[path], DType::F32, device) }.map_err(|e| failed(&e))
}

impl ClipText {
    pub(crate) fn load(dir: &Path) -> EmbedResult<Self> {
        let config: DistilBertConfig =
            serde_json::from_str(&std::fs::read_to_string(dir.join("mclip/config.json")).map_err(|e| failed(&e))?)
                .map_err(|e| failed(&e))?;
        let device = Device::Cpu;
        let model = DistilBertModel::load(weights(&dir.join(crate::prepare::MCLIP_WEIGHTS), &device)?, &config)
            .map_err(|e| failed(&e))?;
        let dense_weights = weights(&dir.join("mclip/dense.safetensors"), &device)?;
        let dense = Linear::new(
            dense_weights.get((CLIP_DIMENSIONS, MCLIP_HIDDEN), "linear.weight").map_err(|e| failed(&e))?,
            None,
        );
        Ok(Self { model, dense, tokenizer: tokenizer(&dir.join("mclip/tokenizer.json"), MAX_TOKENS)?, device })
    }

    pub(crate) fn embed(&self, text: &str) -> EmbedResult<Vec<f32>> {
        let encodings = self.tokenizer.encode_batch(vec![text.to_owned()], true).map_err(|e| failed(&e))?;
        let ids = stack(&encodings, |e| e.get_ids(), &self.device)?;
        let mask = stack(&encodings, |e| e.get_attention_mask(), &self.device)?;
        // candle's DistilBERT masks where the mask is set: 1 marks padding.
        let padding = mask.eq(0_u32).map_err(|e| failed(&e))?;
        let (batch, tokens) = padding.dims2().map_err(|e| failed(&e))?;
        let padding = padding.reshape((batch, 1, 1, tokens)).map_err(|e| failed(&e))?;
        let hidden = self.model.forward(&ids, &padding).map_err(|e| failed(&e))?;
        let pooled = mean_pool(&hidden, &mask)?;
        let projected = self.dense.forward(&pooled).map_err(|e| failed(&e))?;
        Ok(normalise(&projected)?.into_iter().next().unwrap_or_default())
    }
}

impl ClipVision {
    pub(crate) fn load(dir: &Path) -> EmbedResult<Self> {
        let device = Device::Cpu;
        let weights = weights(&dir.join(crate::prepare::CLIP_VISION_WEIGHTS), &device)?;
        let vision = ClipVisionTransformer::new(weights.pp("vision_model"), &ClipVisionConfig::vit_base_patch32())
            .map_err(|e| failed(&e))?;
        let projection = Linear::new(
            weights.pp("visual_projection").get((CLIP_DIMENSIONS, VISION_HIDDEN), "weight").map_err(|e| failed(&e))?,
            None,
        );
        Ok(Self { vision, projection, device })
    }

    /// `None` if the file isn't an image this build can decode.
    pub(crate) fn embed(&self, path: &Path) -> EmbedResult<Option<Vec<f32>>> {
        let Some(pixels) = preprocess(path, &self.device)? else { return Ok(None) };
        let pooled = self.vision.forward(&pixels).map_err(|e| failed(&e))?;
        let projected = self.projection.forward(&pooled).map_err(|e| failed(&e))?;
        Ok(normalise(&projected)?.into_iter().next())
    }
}

/// Resize the short side to 224 (bicubic), centre-crop 224×224, scale to
/// 0..1 and normalise per channel: CLIP's own preprocessing.
fn preprocess(path: &Path, device: &Device) -> EmbedResult<Option<Tensor>> {
    let Ok(image) =
        image::ImageReader::open(path).and_then(|r| r.with_guessed_format()).map(image::ImageReader::decode)
    else {
        return Ok(None);
    };
    let Ok(image) = image else { return Ok(None) };
    let (width, height) = (image.width().max(1), image.height().max(1));
    let scale = f64::from(IMAGE_SIZE) / f64::from(width.min(height));
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "positive and bounded by the image size"
    )]
    let (resized_w, resized_h) = (
        ((f64::from(width) * scale).round() as u32).max(IMAGE_SIZE),
        ((f64::from(height) * scale).round() as u32).max(IMAGE_SIZE),
    );
    let resized = image.resize_exact(resized_w, resized_h, image::imageops::FilterType::CatmullRom);
    let cropped =
        resized.crop_imm((resized_w - IMAGE_SIZE) / 2, (resized_h - IMAGE_SIZE) / 2, IMAGE_SIZE, IMAGE_SIZE).to_rgb8();
    let side = IMAGE_SIZE as usize;
    let mut data = vec![0.0_f32; 3 * side * side];
    for (x, y, pixel) in cropped.enumerate_pixels() {
        for channel in 0..3 {
            let value = f32::from(pixel[channel]) / 255.0;
            data[channel * side * side + y as usize * side + x as usize] = (value - MEAN[channel]) / STD[channel];
        }
    }
    Tensor::from_vec(data, (1, 3, side, side), device).map(Some).map_err(|e| failed(&e))
}
