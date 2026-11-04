//! After download: weights to f16 (half the disk; loaded back as f32), and
//! only CLIP's vision tower and projection kept. Originals are deleted.

use std::collections::HashMap;
use std::path::Path;

use application::ports::ModelPack;
use candle_core::{DType, Device, Tensor};
use domain::VfsError;

pub(crate) const E5_WEIGHTS: &str = "e5/model.f16.safetensors";
pub(crate) const MCLIP_WEIGHTS: &str = "mclip/model.f16.safetensors";
pub(crate) const CLIP_VISION_WEIGHTS: &str = "clip/vision.f16.safetensors";
const E5_ORIGINAL: &str = "e5/model.safetensors";
const MCLIP_ORIGINAL: &str = "mclip/model.safetensors";
const CLIP_ORIGINAL: &str = "clip/pytorch_model.bin";
const VISION_PREFIXES: [&str; 2] = ["vision_model.", "visual_projection."];

fn failed(error: &dyn std::fmt::Display) -> VfsError {
    VfsError::Io { message: format!("preparing the model: {error}") }
}

pub(crate) fn prepared(pack: ModelPack, dir: &Path) -> bool {
    match pack {
        ModelPack::Text => dir.join(E5_WEIGHTS).exists(),
        ModelPack::Images => dir.join(MCLIP_WEIGHTS).exists() && dir.join(CLIP_VISION_WEIGHTS).exists(),
    }
}

pub(crate) fn prepare(pack: ModelPack, dir: &Path) -> Result<(), VfsError> {
    match pack {
        ModelPack::Text => convert_safetensors(&dir.join(E5_ORIGINAL), &dir.join(E5_WEIGHTS)),
        ModelPack::Images => {
            convert_safetensors(&dir.join(MCLIP_ORIGINAL), &dir.join(MCLIP_WEIGHTS))?;
            extract_vision(&dir.join(CLIP_ORIGINAL), &dir.join(CLIP_VISION_WEIGHTS))
        }
    }
}

fn to_f16(tensors: impl IntoIterator<Item = (String, Tensor)>) -> Result<HashMap<String, Tensor>, VfsError> {
    tensors
        .into_iter()
        .map(|(name, tensor)| {
            let converted = if tensor.dtype() == DType::F32 {
                tensor.to_dtype(DType::F16).map_err(|e| failed(&e))?
            } else {
                tensor
            };
            Ok((name, converted))
        })
        .collect()
}

fn save_and_remove(tensors: &HashMap<String, Tensor>, original: &Path, prepared: &Path) -> Result<(), VfsError> {
    let partial = prepared.with_extension("partial");
    candle_core::safetensors::save(tensors, &partial).map_err(|e| failed(&e))?;
    std::fs::rename(&partial, prepared).map_err(|e| failed(&e))?;
    std::fs::remove_file(original).map_err(|e| failed(&e))
}

fn convert_safetensors(original: &Path, prepared: &Path) -> Result<(), VfsError> {
    if prepared.exists() {
        return Ok(());
    }
    let tensors = candle_core::safetensors::load(original, &Device::Cpu).map_err(|e| failed(&e))?;
    save_and_remove(&to_f16(tensors)?, original, prepared)
}

/// CLIP is published as one PyTorch pickle; candle reads it with a
/// restricted loader that only rebuilds tensors (no code runs).
fn extract_vision(original: &Path, prepared: &Path) -> Result<(), VfsError> {
    if prepared.exists() {
        return Ok(());
    }
    let tensors = candle_core::pickle::read_all(original).map_err(|e| failed(&e))?;
    let vision = tensors.into_iter().filter(|(name, _)| VISION_PREFIXES.iter().any(|p| name.starts_with(p)));
    save_and_remove(&to_f16(vision)?, original, prepared)
}
