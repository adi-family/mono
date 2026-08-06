// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::error::{Error, Result};
use bytemuck::{Pod, Zeroable};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

const MAGIC: [u8; 4] = *b"ADIE"; // ADI Embeddings
const VERSION: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Header {
    magic: [u8; 4],
    version: u32,
    dimensions: u32,
    _padding: u32, // Align count to 8 bytes
    count: u64,
    model_hash: [u8; 32],
}

#[derive(Debug)]
pub struct EmbeddingStore {
    path: std::path::PathBuf,
    dimensions: u32,
    #[allow(dead_code)]
    model_hash: [u8; 32],
}

impl EmbeddingStore {
    pub fn create(path: &Path, dimensions: u32, model_hash: [u8; 32]) -> Result<Self> {
        let header = Header {
            magic: MAGIC,
            version: VERSION,
            dimensions,
            _padding: 0,
            count: 0,
            model_hash,
        };

        let mut file = File::create(path)?;
        file.write_all(bytemuck::bytes_of(&header))?;

        Ok(Self {
            path: path.to_path_buf(),
            dimensions,
            model_hash,
        })
    }

    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let mut header_bytes = [0u8; std::mem::size_of::<Header>()];
        file.read_exact(&mut header_bytes)?;

        let header: Header = *bytemuck::from_bytes(&header_bytes);

        if header.magic != MAGIC {
            return Err(Error::Storage("Invalid embedding file magic".to_string()));
        }

        if header.version != VERSION {
            return Err(Error::Storage(format!(
                "Unsupported embedding file version: {}",
                header.version
            )));
        }

        Ok(Self {
            path: path.to_path_buf(),
            dimensions: header.dimensions,
            model_hash: header.model_hash,
        })
    }

    pub fn open_or_create(path: &Path, dimensions: u32, model_hash: [u8; 32]) -> Result<Self> {
        if path.exists() {
            Self::open(path)
        } else {
            Self::create(path, dimensions, model_hash)
        }
    }

    #[must_use]
    pub fn dimensions(&self) -> u32 {
        self.dimensions
    }

    pub fn count(&self) -> Result<u64> {
        let file = File::open(&self.path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len();
        let header_size = std::mem::size_of::<Header>() as u64;
        let embedding_size = u64::from(self.dimensions) * 4; // f32 = 4 bytes

        Ok((file_size - header_size) / embedding_size)
    }

    pub fn append(&self, embeddings: &[Vec<f32>]) -> Result<()> {
        if embeddings.is_empty() {
            return Ok(());
        }

        let mut file = OpenOptions::new().append(true).open(&self.path)?;

        for embedding in embeddings {
            if embedding.len() != self.dimensions as usize {
                return Err(Error::Storage(format!(
                    "Embedding dimension mismatch: expected {}, got {}",
                    self.dimensions,
                    embedding.len()
                )));
            }
            file.write_all(bytemuck::cast_slice(embedding))?;
        }

        // Update count in header
        self.update_count()?;

        Ok(())
    }

    fn update_count(&self) -> Result<()> {
        let count = self.count()?;

        let mut file = OpenOptions::new().write(true).open(&self.path)?;

        // Write count at offset 16 (after magic + version + dimensions + padding)
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(16))?;
        file.write_all(&count.to_le_bytes())?;

        Ok(())
    }

    pub fn get(&self, index: u64) -> Result<Vec<f32>> {
        use std::io::Seek;
        let mut file = File::open(&self.path)?;
        let file_len = file.metadata()?.len();

        let header_size = std::mem::size_of::<Header>() as u64;
        let embedding_size = u64::from(self.dimensions) * 4;
        let offset = header_size + index * embedding_size;

        if offset + embedding_size > file_len {
            return Err(Error::Storage(format!(
                "Embedding index {index} out of bounds"
            )));
        }

        file.seek(std::io::SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; embedding_size as usize];
        file.read_exact(&mut buf)?;
        let floats: &[f32] = bytemuck::cast_slice(&buf);

        Ok(floats.to_vec())
    }

    pub fn get_batch(&self, indices: &[u64]) -> Result<Vec<Vec<f32>>> {
        use std::io::Seek;
        let mut file = File::open(&self.path)?;
        let file_len = file.metadata()?.len();

        let header_size = std::mem::size_of::<Header>() as u64;
        let embedding_size = u64::from(self.dimensions) * 4;

        let mut results = Vec::with_capacity(indices.len());

        for &index in indices {
            let offset = header_size + index * embedding_size;

            if offset + embedding_size > file_len {
                return Err(Error::Storage(format!(
                    "Embedding index {index} out of bounds"
                )));
            }

            file.seek(std::io::SeekFrom::Start(offset))?;
            let mut buf = vec![0u8; embedding_size as usize];
            file.read_exact(&mut buf)?;
            let floats: &[f32] = bytemuck::cast_slice(&buf);
            results.push(floats.to_vec());
        }

        Ok(results)
    }

    pub fn iter(&self) -> Result<EmbeddingIterator> {
        use std::io::Seek;
        let mut file = File::open(&self.path)?;
        let count = self.count()?;
        let header_size = std::mem::size_of::<Header>() as u64;
        file.seek(std::io::SeekFrom::Start(header_size))?;

        Ok(EmbeddingIterator {
            file,
            dimensions: self.dimensions as usize,
            count: count as usize,
            current: 0,
        })
    }
}

#[derive(Debug)]
pub struct EmbeddingIterator {
    file: File,
    dimensions: usize,
    count: usize,
    current: usize,
}

impl Iterator for EmbeddingIterator {
    type Item = Vec<f32>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current >= self.count {
            return None;
        }

        let embedding_size = self.dimensions * 4;
        let mut buf = vec![0u8; embedding_size];
        self.file.read_exact(&mut buf).ok()?;
        let floats: &[f32] = bytemuck::cast_slice(&buf);

        self.current += 1;

        Some(floats.to_vec())
    }
}
