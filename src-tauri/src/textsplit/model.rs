//! ONNX model wrapper for the vendored SaT port.
//!
//! Ported from wtsplit-rs `model.rs` (SaT path only — WtP dropped) and adapted
//! from `ort` rc.9 to the rc.12 API already in the tree (via transcribe-rs):
//! - `Value::from_array` → `ort::value::Tensor::from_array`
//! - session outputs are `DynValue`s; extracted with `try_extract_array`
//! - logits are read flexibly (f16 first, f32 fallback) so both the fp16
//!   `model_optimized.onnx` and a hypothetical f32 export work unchanged.

use half::f16;
use ndarray::{Array2, Array3, ArrayView2};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

use crate::textsplit::config::ModelConfig;
use crate::textsplit::Result;

/// ONNX model wrapper for SaT (subword) inference.
pub struct OnnxModel {
    session: Session,
    pub config: ModelConfig,
}

impl OnnxModel {
    /// Load an ONNX model from a file.
    pub fn new(onnx_path: &Path, config: ModelConfig) -> Result<Self> {
        let session = Session::builder()
            .map_err(|e| format!("Failed to create ONNX session builder: {}", e))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| format!("Failed to set optimization level: {}", e))?
            .commit_from_file(onnx_path)
            .map_err(|e| format!("Failed to load ONNX model {:?}: {}", onnx_path, e))?;

        Ok(Self { session, config })
    }

    /// Run inference for a SaT model (subword-based).
    ///
    /// `input_ids`/`attention_mask` are (batch x seq). Returns logits
    /// (batch x seq x num_labels) as f32.
    pub fn forward_sat(
        &mut self,
        input_ids: ArrayView2<'_, i64>,
        attention_mask: ArrayView2<'_, f32>,
    ) -> Result<Array3<f32>> {
        // input_ids stay int64 (token indices).
        let input_ids_value = Tensor::from_array(input_ids.to_owned().as_standard_layout().to_owned())
            .map_err(|e| format!("Failed to build input_ids tensor: {}", e))?;

        // The SaT ONNX export is fp16, so the attention mask must be f16 too.
        let attention_mask_f16: Array2<f16> = attention_mask.mapv(f16::from_f32);
        let attention_mask_value =
            Tensor::from_array(attention_mask_f16.as_standard_layout().to_owned())
                .map_err(|e| format!("Failed to build attention_mask tensor: {}", e))?;

        let outputs = self
            .session
            .run(ort::inputs![
                "input_ids" => input_ids_value,
                "attention_mask" => attention_mask_value
            ])
            .map_err(|e| format!("Inference failed: {}", e))?;

        let logits = &outputs["logits"];

        // Prefer f16 (the shipped optimized model); fall back to f32.
        let (dims, data_f32): (Vec<usize>, Vec<f32>) = match logits.try_extract_array::<f16>() {
            Ok(view) => {
                let dims = view.shape().to_vec();
                let data = view.iter().map(|x| x.to_f32()).collect();
                (dims, data)
            }
            Err(_) => {
                let view = logits
                    .try_extract_array::<f32>()
                    .map_err(|e| format!("Failed to extract logits tensor: {}", e))?;
                let dims = view.shape().to_vec();
                let data = view.iter().copied().collect();
                (dims, data)
            }
        };

        if dims.len() != 3 {
            return Err(format!("Unexpected logits rank: {:?} (expected [batch, seq, labels])", dims));
        }

        Array3::from_shape_vec((dims[0], dims[1], dims[2]), data_f32)
            .map_err(|e| format!("Failed to reshape logits: {}", e))
    }
}
