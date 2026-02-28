use tracing::debug;

/// Splits a dataset into shards for data-parallel training.
pub struct DataShardSplitter;

impl DataShardSplitter {
    /// Split data indices into N equal shards.
    pub fn split(total_samples: usize, num_shards: usize) -> Vec<DataShard> {
        if num_shards == 0 {
            return Vec::new();
        }

        let per_shard = total_samples / num_shards;
        let mut remainder = total_samples % num_shards;

        let mut shards = Vec::with_capacity(num_shards);
        let mut offset = 0;

        for i in 0..num_shards {
            let mut count = per_shard;
            if remainder > 0 {
                count += 1;
                remainder -= 1;
            }

            shards.push(DataShard {
                shard_index: i,
                start_index: offset,
                count,
            });

            debug!("Data shard {}: offset={} count={}", i, offset, count);
            offset += count;
        }

        shards
    }
}

/// A contiguous range of data samples.
#[derive(Debug, Clone)]
pub struct DataShard {
    pub shard_index: usize,
    pub start_index: usize,
    pub count: usize,
}

/// Aggregate gradients from multiple nodes using element-wise averaging.
pub fn aggregate_gradients(all_gradients: &[Vec<f32>]) -> Vec<f32> {
    if all_gradients.is_empty() {
        return Vec::new();
    }

    let len = all_gradients[0].len();
    let n = all_gradients.len() as f32;

    let mut result = vec![0.0f32; len];

    for gradients in all_gradients {
        assert_eq!(gradients.len(), len, "Gradient dimension mismatch");
        for (i, g) in gradients.iter().enumerate() {
            result[i] += g;
        }
    }

    for val in &mut result {
        *val /= n;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_even() {
        let shards = DataShardSplitter::split(100, 4);
        assert_eq!(shards.len(), 4);

        let total: usize = shards.iter().map(|s| s.count).sum();
        assert_eq!(total, 100);

        assert_eq!(shards[0].count, 25);
    }

    #[test]
    fn test_split_uneven() {
        let shards = DataShardSplitter::split(10, 3);
        assert_eq!(shards.len(), 3);

        let total: usize = shards.iter().map(|s| s.count).sum();
        assert_eq!(total, 10);

        // First shard gets extra
        assert_eq!(shards[0].count, 4);
        assert_eq!(shards[1].count, 3);
        assert_eq!(shards[2].count, 3);
    }

    #[test]
    fn test_aggregate_gradients() {
        let grads = vec![
            vec![2.0, 4.0, 6.0],
            vec![4.0, 6.0, 8.0],
        ];

        let result = aggregate_gradients(&grads);
        assert_eq!(result, vec![3.0, 5.0, 7.0]);
    }

    #[test]
    fn test_aggregate_gradients_empty() {
        let result = aggregate_gradients(&[]);
        assert!(result.is_empty());
    }
}
