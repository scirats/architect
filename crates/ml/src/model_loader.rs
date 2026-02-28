use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Tensor {
    pub data: Vec<f32>,
    pub shape: Vec<usize>,
}

/// Load all tensors from a .safetensors file.
pub fn load_safetensors(path: &Path) -> anyhow::Result<HashMap<String, Tensor>> {
    let data = std::fs::read(path)?;
    let tensors = safetensors::SafeTensors::deserialize(&data)?;

    let mut result = HashMap::new();
    for (name, view) in tensors.tensors() {
        let shape: Vec<usize> = view.shape().to_vec();

        // Convert to f32 based on dtype
        let f32_data = match view.dtype() {
            safetensors::Dtype::F32 => {
                // Direct reinterpret
                view.data()
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect()
            }
            safetensors::Dtype::F16 => {
                // Convert f16 to f32
                view.data()
                    .chunks_exact(2)
                    .map(|chunk| {
                        let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
                        half_to_f32(bits)
                    })
                    .collect()
            }
            safetensors::Dtype::BF16 => {
                view.data()
                    .chunks_exact(2)
                    .map(|chunk| {
                        let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
                        bf16_to_f32(bits)
                    })
                    .collect()
            }
            other => anyhow::bail!("Unsupported dtype: {:?}", other),
        };

        result.insert(
            name.to_string(),
            Tensor {
                data: f32_data,
                shape,
            },
        );
    }

    Ok(result)
}

/// Extract weight tensors for a range of layers.
pub fn extract_layer_weights(
    tensors: &HashMap<String, Tensor>,
    layer_start: u32,
    layer_end: u32,
) -> Vec<Vec<f32>> {
    let mut weights = Vec::new();
    for layer_idx in layer_start..layer_end {
        // Common naming patterns
        let patterns = [
            format!("layers.{}.weight", layer_idx),
            format!("model.layers.{}.mlp.weight", layer_idx),
            format!("model.layers.{}.self_attn.weight", layer_idx),
            format!("h.{}.weight", layer_idx),
        ];
        for pattern in &patterns {
            for (name, tensor) in tensors {
                if name.contains(pattern) {
                    weights.push(tensor.data.clone());
                }
            }
        }
    }
    weights
}

/// List all tensor names (for debugging).
pub fn tensor_names(tensors: &HashMap<String, Tensor>) -> Vec<String> {
    let mut names: Vec<String> = tensors.keys().cloned().collect();
    names.sort();
    names
}

// f16 to f32 conversion
fn half_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let frac = (h & 0x3ff) as u32;

    if exp == 0 {
        if frac == 0 {
            return f32::from_bits(sign << 31);
        }
        // Subnormal
        let mut e = 0i32;
        let mut f = frac;
        while (f & 0x400) == 0 {
            f <<= 1;
            e -= 1;
        }
        f &= 0x3ff;
        let exp32 = (127 - 15 + 1 + e) as u32;
        return f32::from_bits((sign << 31) | (exp32 << 23) | (f << 13));
    } else if exp == 31 {
        if frac == 0 {
            return f32::from_bits((sign << 31) | (0xff << 23));
        }
        return f32::NAN;
    }

    let exp32 = exp - 15 + 127;
    f32::from_bits((sign << 31) | (exp32 << 23) | (frac << 13))
}

// bf16 to f32 conversion
fn bf16_to_f32(b: u16) -> f32 {
    f32::from_bits((b as u32) << 16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_half_to_f32() {
        // 1.0 in f16 = 0x3C00
        let val = half_to_f32(0x3C00);
        assert!((val - 1.0).abs() < 1e-6);

        // 0.0 in f16 = 0x0000
        let val = half_to_f32(0x0000);
        assert_eq!(val, 0.0);
    }

    #[test]
    fn test_bf16_to_f32() {
        // 1.0 in bf16 = 0x3F80
        let val = bf16_to_f32(0x3F80);
        assert!((val - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_tensor_names() {
        let mut tensors = HashMap::new();
        tensors.insert(
            "layer.0.weight".to_string(),
            Tensor {
                data: vec![1.0],
                shape: vec![1],
            },
        );
        tensors.insert(
            "layer.1.weight".to_string(),
            Tensor {
                data: vec![2.0],
                shape: vec![1],
            },
        );
        let names = tensor_names(&tensors);
        assert_eq!(names.len(), 2);
    }
}
