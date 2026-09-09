use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::rules::Rules;
use crate::search::SearchMeta;
use crate::tt::TranspositionTable;

/// Checkpoint file header.
///
/// TT data follows immediately after header.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointHeader {
    pub magic: [u8; 8],
    pub version: u32,

    pub rules: Rules,
    pub meta: SearchMeta,

    pub shards: u32,
    pub tt_entries: u64,
}

const MAGIC: [u8; 8] = *b"BSRKTT03";
const VERSION: u32 = 3;

pub fn save_checkpoint(
    path: &Path,
    compress: bool,
    rules: &Rules,
    meta: SearchMeta,
    tt: &TranspositionTable,
) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let f = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    let buf = BufWriter::new(f);

    if compress {
        // Stream-compress with zstd.
        let mut enc =
            zstd::stream::write::Encoder::new(buf, 3).context("zstd Encoder::new failed")?;
        write_checkpoint_stream(&mut enc, rules, meta, tt)?;
        let mut buf = enc.finish().context("zstd finish failed")?;
        buf.flush()?;
        buf.get_ref().sync_all()?;
    } else {
        let mut w = buf;
        write_checkpoint_stream(&mut w, rules, meta, tt)?;
        w.flush()?;
        w.get_ref().sync_all()?;
    }

    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;

    Ok(())
}

fn write_checkpoint_stream<W: std::io::Write>(
    w: &mut W,
    rules: &Rules,
    meta: SearchMeta,
    tt: &TranspositionTable,
) -> Result<()> {
    let header = CheckpointHeader {
        magic: MAGIC,
        version: VERSION,
        rules: *rules,
        meta,
        shards: TranspositionTable::SHARDS as u32,
        tt_entries: tt.total_len(),
    };

    bincode::serialize_into(&mut *w, &header)?;
    tt.write_to(w)?;
    Ok(())
}

pub fn load_checkpoint(
    path: &Path,
    no_compress: bool,
) -> Result<(Rules, SearchMeta, TranspositionTable)> {
    let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let buf = BufReader::new(f);

    if no_compress {
        read_checkpoint_stream(buf)
    } else {
        let dec = zstd::stream::read::Decoder::new(buf).context("zstd Decoder::new failed")?;
        read_checkpoint_stream(dec)
    }
}

fn read_checkpoint_stream<R: std::io::Read>(
    mut r: R,
) -> Result<(Rules, SearchMeta, TranspositionTable)> {
    let header: CheckpointHeader = bincode::deserialize_from(&mut r)?;
    if header.magic != MAGIC {
        anyhow::bail!("Invalid checkpoint magic");
    }
    if header.version != VERSION {
        anyhow::bail!("Unsupported checkpoint version {}", header.version);
    }
    if header.shards as usize != TranspositionTable::SHARDS {
        anyhow::bail!(
            "Shard count mismatch: file has {}, binary expects {}",
            header.shards,
            TranspositionTable::SHARDS
        );
    }

    let mut tt = TranspositionTable::new(header.tt_entries as usize);
    tt.read_from(&mut r)?;

    Ok((header.rules, header.meta, tt))
}
