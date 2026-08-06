// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! Candle-based embedder with Metal GPU support.
//!
//! Custom `JinaBert` implementation with QK-norm support, which candle-transformers lacks.
//! The jina-embeddings-v2-base-code model requires QK-norm for correct attention scores.

use super::error::{EmbedError, Result};
use super::Embedder;
use candle_core::{DType, Device, IndexOp, Module, Tensor, D};
use candle_nn::{layer_norm, linear, linear_no_bias, LayerNorm, Linear, VarBuilder};
use candle_transformers::models::jina_bert::Config;
use std::sync::Mutex;
use tokenizers::Tokenizer;

const MODEL_ID: &str = "jinaai/jina-embeddings-v2-base-code";
const DIMENSIONS: u32 = 768;

/// Tokens per symbol, past which the text is truncated.
///
/// The model reads 8192, but a batch pads to its longest member and attention costs the square
/// of that — so the price of allowing one long symbol is paid, squared, by every symbol batched
/// with it. 512 covers a whole function in the large majority of cases and keeps the worst
/// batch to something a GPU returns from promptly.
const MAX_TOKENS: usize = 512;

// --- Custom JinaBert model with QK-norm ---

struct BertEmbeddings {
    word_embeddings: candle_nn::Embedding,
    token_type_embeddings: candle_nn::Embedding,
    layer_norm: LayerNorm,
}

impl BertEmbeddings {
    fn load(vb: VarBuilder, cfg: &Config) -> candle_core::Result<Self> {
        Ok(Self {
            word_embeddings: candle_nn::embedding(
                cfg.vocab_size,
                cfg.hidden_size,
                vb.pp("word_embeddings"),
            )?,
            token_type_embeddings: candle_nn::embedding(
                cfg.type_vocab_size,
                cfg.hidden_size,
                vb.pp("token_type_embeddings"),
            )?,
            layer_norm: layer_norm(cfg.hidden_size, cfg.layer_norm_eps, vb.pp("LayerNorm"))?,
        })
    }

    fn forward(&self, input_ids: &Tensor) -> candle_core::Result<Tensor> {
        let (b, seq_len) = input_ids.dims2()?;
        let word_emb = self.word_embeddings.forward(input_ids)?;
        let tt_ids = Tensor::zeros(seq_len, DType::U32, input_ids.device())?.broadcast_left(b)?;
        let tt_emb = self.token_type_embeddings.forward(&tt_ids)?;
        self.layer_norm.forward(&word_emb.add(&tt_emb)?)
    }
}

struct BertSelfAttention {
    query: Linear,
    key: Linear,
    value: Linear,
    layer_norm_q: LayerNorm,
    layer_norm_k: LayerNorm,
    num_heads: usize,
    head_size: usize,
}

impl BertSelfAttention {
    fn load(vb: VarBuilder, cfg: &Config) -> candle_core::Result<Self> {
        let head_size = cfg.hidden_size / cfg.num_attention_heads;
        let all_head_size = cfg.num_attention_heads * head_size;
        Ok(Self {
            query: linear(cfg.hidden_size, all_head_size, vb.pp("query"))?,
            key: linear(cfg.hidden_size, all_head_size, vb.pp("key"))?,
            value: linear(cfg.hidden_size, all_head_size, vb.pp("value"))?,
            layer_norm_q: layer_norm(all_head_size, cfg.layer_norm_eps, vb.pp("layer_norm_q"))?,
            layer_norm_k: layer_norm(all_head_size, cfg.layer_norm_eps, vb.pp("layer_norm_k"))?,
            num_heads: cfg.num_attention_heads,
            head_size,
        })
    }

    fn reshape_heads(&self, xs: &Tensor) -> candle_core::Result<Tensor> {
        let mut shape = xs.dims().to_vec();
        shape.pop();
        shape.push(self.num_heads);
        shape.push(self.head_size);
        xs.reshape(shape)?.transpose(1, 2)?.contiguous()
    }

    fn forward(&self, xs: &Tensor, alibi_bias: &Tensor) -> candle_core::Result<Tensor> {
        let q = self.layer_norm_q.forward(&self.query.forward(xs)?)?;
        let k = self.layer_norm_k.forward(&self.key.forward(xs)?)?;
        let v = self.value.forward(xs)?;

        let q = self.reshape_heads(&q)?;
        let k = self.reshape_heads(&k)?;
        let v = self.reshape_heads(&v)?;

        let scores = q.matmul(&k.t()?)?;
        let scores = (scores / (self.head_size as f64).sqrt())?;
        let scores = scores.broadcast_add(alibi_bias)?;
        let probs = candle_nn::ops::softmax_last_dim(&scores)?;
        let ctx = probs.matmul(&v)?;
        ctx.transpose(1, 2)?.contiguous()?.flatten_from(D::Minus2)
    }
}

struct BertSelfOutput {
    dense: Linear,
    layer_norm: LayerNorm,
}

impl BertSelfOutput {
    fn load(vb: VarBuilder, cfg: &Config) -> candle_core::Result<Self> {
        Ok(Self {
            dense: linear(cfg.hidden_size, cfg.hidden_size, vb.pp("dense"))?,
            layer_norm: layer_norm(cfg.hidden_size, cfg.layer_norm_eps, vb.pp("LayerNorm"))?,
        })
    }

    fn forward(&self, xs: &Tensor, residual: &Tensor) -> candle_core::Result<Tensor> {
        self.layer_norm
            .forward(&self.dense.forward(xs)?.add(residual)?)
    }
}

struct BertAttention {
    self_attn: BertSelfAttention,
    output: BertSelfOutput,
}

impl BertAttention {
    fn load(vb: VarBuilder, cfg: &Config) -> candle_core::Result<Self> {
        Ok(Self {
            self_attn: BertSelfAttention::load(vb.pp("self"), cfg)?,
            output: BertSelfOutput::load(vb.pp("output"), cfg)?,
        })
    }

    fn forward(&self, xs: &Tensor, alibi_bias: &Tensor) -> candle_core::Result<Tensor> {
        let attn_out = self.self_attn.forward(xs, alibi_bias)?;
        self.output.forward(&attn_out, xs)
    }
}

struct BertGLUMLP {
    up_gated_layer: Linear,
    down_layer: Linear,
    intermediate_size: usize,
}

impl BertGLUMLP {
    fn load(vb: VarBuilder, cfg: &Config) -> candle_core::Result<Self> {
        Ok(Self {
            up_gated_layer: linear_no_bias(
                cfg.hidden_size,
                cfg.intermediate_size * 2,
                vb.pp("up_gated_layer"),
            )?,
            down_layer: linear(cfg.intermediate_size, cfg.hidden_size, vb.pp("down_layer"))?,
            intermediate_size: cfg.intermediate_size,
        })
    }

    fn forward(&self, xs: &Tensor) -> candle_core::Result<Tensor> {
        let projected = self.up_gated_layer.forward(xs)?;
        let up = projected.narrow(D::Minus1, 0, self.intermediate_size)?;
        let gated = projected.narrow(D::Minus1, self.intermediate_size, self.intermediate_size)?;
        let activated = (up * gated.gelu()?)?;
        self.down_layer.forward(&activated)
    }
}

struct BertLayer {
    attention: BertAttention,
    layer_norm_1: LayerNorm,
    layer_norm_2: LayerNorm,
    mlp: BertGLUMLP,
}

impl BertLayer {
    fn load(vb: VarBuilder, cfg: &Config) -> candle_core::Result<Self> {
        Ok(Self {
            attention: BertAttention::load(vb.pp("attention"), cfg)?,
            layer_norm_1: layer_norm(cfg.hidden_size, cfg.layer_norm_eps, vb.pp("layer_norm_1"))?,
            layer_norm_2: layer_norm(cfg.hidden_size, cfg.layer_norm_eps, vb.pp("layer_norm_2"))?,
            mlp: BertGLUMLP::load(vb.pp("mlp"), cfg)?,
        })
    }

    fn forward(&self, xs: &Tensor, alibi_bias: &Tensor) -> candle_core::Result<Tensor> {
        let attn_out = self.attention.forward(xs, alibi_bias)?;
        let residual = self.layer_norm_1.forward(&xs.add(&attn_out)?)?;
        let mlp_out = self.mlp.forward(&residual)?;
        self.layer_norm_2.forward(&residual.add(&mlp_out)?)
    }
}

fn build_alibi_bias(cfg: &Config, device: &Device) -> candle_core::Result<Tensor> {
    let n_heads = cfg.num_attention_heads;
    let seq_len = cfg.max_position_embeddings;
    let positions = Tensor::arange(0, seq_len as i64, &Device::Cpu)?.to_dtype(DType::F32)?;
    let bias = {
        let a1 = positions.reshape((1, seq_len))?;
        let a2 = positions.reshape((seq_len, 1))?;
        a1.broadcast_sub(&a2)?.abs()?.broadcast_left(n_heads)?
    };
    let mut n2 = 1;
    while n2 < n_heads {
        n2 *= 2;
    }
    let slopes: Vec<f32> = (1..=n2)
        .map(|v| -1f32 / 2f32.powf((v * 8) as f32 / n2 as f32))
        .collect();
    let slopes = if n2 == n_heads {
        slopes
    } else {
        slopes
            .iter()
            .skip(1)
            .step_by(2)
            .chain(slopes.iter().step_by(2))
            .take(n_heads)
            .copied()
            .collect()
    };
    let slopes = Tensor::new(slopes, &Device::Cpu)?.reshape((1, (), 1, 1))?;
    bias.broadcast_mul(&slopes)?.to_device(device)
}

struct JinaBertModel {
    embeddings: BertEmbeddings,
    layers: Vec<BertLayer>,
    alibi_bias: Tensor,
}

impl JinaBertModel {
    fn load(vb: VarBuilder, cfg: &Config) -> candle_core::Result<Self> {
        let embeddings = BertEmbeddings::load(vb.pp("embeddings"), cfg)?;
        let encoder_vb = vb.pp("encoder");
        let layers = (0..cfg.num_hidden_layers)
            .map(|i| BertLayer::load(encoder_vb.pp(format!("layer.{i}")), cfg))
            .collect::<candle_core::Result<Vec<_>>>()?;
        let alibi_bias = build_alibi_bias(cfg, vb.device())?;
        Ok(Self {
            embeddings,
            layers,
            alibi_bias,
        })
    }

    fn forward(&self, input_ids: &Tensor) -> candle_core::Result<Tensor> {
        let seq_len = input_ids.dim(1)?;
        let alibi = self.alibi_bias.i((.., .., ..seq_len, ..seq_len))?;
        let mut xs = self.embeddings.forward(input_ids)?;
        for layer in &self.layers {
            xs = layer.forward(&xs, &alibi)?;
        }
        Ok(xs)
    }
}

// --- Embedder ---

pub struct CandleEmbedder {
    model: Mutex<JinaBertModel>,
    tokenizer: Tokenizer,
    device: Device,
}

/// The loaded weights are neither `Debug` nor interesting to print — the model and the device
/// it runs on are.
impl std::fmt::Debug for CandleEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CandleEmbedder")
            .field("model", &MODEL_ID)
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

impl CandleEmbedder {
    pub fn new() -> Result<Self> {
        let device = Self::select_device();
        let device_name = match &device {
            Device::Metal(_) => "Metal GPU",
            Device::Cuda(_) => "CUDA GPU",
            Device::Cpu => "CPU",
        };
        tracing::info!(
            device = device_name,
            model = MODEL_ID,
            "Loading candle embedder"
        );

        let api = hf_hub::api::sync::Api::new()
            .map_err(|e| EmbedError::Embedding(format!("HF Hub init: {e}")))?;
        let repo = api.model(MODEL_ID.to_string());

        let config_path = repo
            .get("config.json")
            .map_err(|e| EmbedError::Embedding(format!("Download config.json: {e}")))?;
        let tokenizer_path = repo
            .get("tokenizer.json")
            .map_err(|e| EmbedError::Embedding(format!("Download tokenizer.json: {e}")))?;
        let weights_path = repo
            .get("model.safetensors")
            .map_err(|e| EmbedError::Embedding(format!("Download model.safetensors: {e}")))?;

        let config: Config = serde_json::from_reader(
            std::fs::File::open(&config_path)
                .map_err(|e| EmbedError::Embedding(format!("Open config: {e}")))?,
        )
        .map_err(|e| EmbedError::Embedding(format!("Parse config: {e}")))?;

        let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| EmbedError::Embedding(format!("Load tokenizer: {e}")))?;

        // Without this a long symbol is a hard error, not a long embedding: the ALiBi bias is
        // precomputed at `max_position_embeddings` and sliced to the batch's sequence length,
        // so a sequence past that limit fails the slice and takes the whole file's embeddings
        // down with it. `MAX_TOKENS` sits well under the limit — see its own note on why.
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_TOKENS,
                ..Default::default()
            }))
            .map_err(|e| EmbedError::Embedding(format!("Set truncation: {e}")))?;

        // Load weights — tensor names match our model struct directly, no renames needed
        let tensors = candle_core::safetensors::load(&weights_path, &device)
            .map_err(|e| EmbedError::Embedding(format!("Load weights: {e}")))?;

        let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);

        let model = JinaBertModel::load(vb, &config)
            .map_err(|e| EmbedError::Embedding(format!("Build model: {e}")))?;

        tracing::info!(device = device_name, "Candle embedder ready");

        Ok(Self {
            model: Mutex::new(model),
            tokenizer,
            device,
        })
    }

    /// Metal when the build has it and the machine offers it, CPU otherwise. `new_metal`
    /// compiles either way — without the feature (any non-macOS target, see Cargo.toml) it is
    /// the stub that always errors, and this falls through to the CPU.
    fn select_device() -> Device {
        match Device::new_metal(0) {
            Ok(device) => device,
            Err(e) => {
                tracing::debug!("Metal unavailable ({e}), embedding on the CPU");
                Device::Cpu
            }
        }
    }

    fn mean_pool(embeddings: &Tensor, attention_mask: &Tensor) -> Result<Tensor> {
        let mask = attention_mask
            .unsqueeze(2)
            .and_then(|m| m.to_dtype(DType::F32))
            .and_then(|m| m.broadcast_as(embeddings.shape()))
            .map_err(|e| EmbedError::Embedding(format!("Mask expand: {e}")))?;
        let summed = embeddings
            .mul(&mask)
            .and_then(|t| t.sum(1))
            .map_err(|e| EmbedError::Embedding(format!("Sum: {e}")))?;
        let mask_sum = mask
            .sum(1)
            .and_then(|s| s.clamp(1e-9, f64::MAX))
            .map_err(|e| EmbedError::Embedding(format!("Mask sum: {e}")))?;
        summed
            .broadcast_div(&mask_sum)
            .map_err(|e| EmbedError::Embedding(format!("Div: {e}")))
    }

    fn normalize(tensor: &Tensor) -> Result<Tensor> {
        let l2 = tensor
            .sqr()
            .and_then(|t| t.sum_keepdim(1))
            .and_then(|t| t.sqrt())
            .and_then(|t| t.clamp(1e-12, f64::MAX))
            .map_err(|e| EmbedError::Embedding(format!("L2 norm: {e}")))?;
        tensor
            .broadcast_div(&l2)
            .map_err(|e| EmbedError::Embedding(format!("Normalize: {e}")))
    }
}

impl Embedder for CandleEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| EmbedError::Embedding(format!("Tokenize: {e}")))?;

        let max_len = encodings
            .iter()
            .map(|e| e.get_ids().len())
            .max()
            .unwrap_or(0);

        let mut all_ids: Vec<u32> = Vec::with_capacity(texts.len() * max_len);
        let mut all_masks: Vec<u32> = Vec::with_capacity(texts.len() * max_len);

        for enc in &encodings {
            let ids = enc.get_ids();
            let mask = enc.get_attention_mask();
            let pad_len = max_len - ids.len();
            all_ids.extend_from_slice(ids);
            all_ids.extend(std::iter::repeat_n(0u32, pad_len));
            all_masks.extend_from_slice(mask);
            all_masks.extend(std::iter::repeat_n(0u32, pad_len));
        }

        let batch_size = texts.len();
        let input_ids = Tensor::from_vec(all_ids, (batch_size, max_len), &self.device)
            .map_err(|e| EmbedError::Embedding(format!("Input tensor: {e}")))?;
        let attention_mask = Tensor::from_vec(all_masks, (batch_size, max_len), &self.device)
            .map_err(|e| EmbedError::Embedding(format!("Mask tensor: {e}")))?;

        let model = self
            .model
            .lock()
            .map_err(|e| EmbedError::Embedding(format!("Lock: {e}")))?;

        let sequence_output = model
            .forward(&input_ids)
            .map_err(|e| EmbedError::Embedding(format!("Forward: {e}")))?;

        let pooled = Self::mean_pool(&sequence_output, &attention_mask)?;
        let normalized = Self::normalize(&pooled)?;

        normalized
            .to_vec2::<f32>()
            .map_err(|e| EmbedError::Embedding(format!("To vec: {e}")))
    }

    fn dimensions(&self) -> u32 {
        DIMENSIONS
    }

    fn model_name(&self) -> &str {
        MODEL_ID
    }
}
