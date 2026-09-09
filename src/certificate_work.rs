//! Durable, explicitly UNVERIFIED certificate construction work.
//! Only Certificate::verify can turn this work into a proof.
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use anyhow::{ensure, Context, Result};

use crate::certificate::{code, decode, Certificate, Header, Node, ALL_MOVES, MAX_NODES};

const MAGIC: &[u8; 8] = b"BRCWORK1";
const MAX_PENDING: usize = 512 * 36 + 1;

pub struct Work {
    pub certificate: Certificate,
    pub pending: Vec<(u64, u16)>,
    pub visited: u64,
    pub implicit: u64,
}

impl Work {
    pub fn new(header: Header) -> Self {
        let root = (
            code(header.start_key, header.first),
            header.forced_depth.unwrap_or(0),
        );
        Self {
            certificate: Certificate {
                header,
                nodes: HashMap::new(),
            },
            pending: vec![root],
            visited: 0,
            implicit: 0,
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        ensure!(
            self.certificate.nodes.len() <= MAX_NODES && self.pending.len() <= MAX_PENDING,
            "certificate work exceeds storage budget"
        );
        let tmp = path.with_extension("certificate-work.tmp");
        let mut w = BufWriter::new(File::create(&tmp)?);
        w.write_all(MAGIC)?;
        bincode::serialize_into(&mut w, &self.certificate.header)?;
        bincode::serialize_into(&mut w, &(self.certificate.nodes.len() as u64))?;
        for (&state, &node) in &self.certificate.nodes {
            bincode::serialize_into(&mut w, &(state, node.rank, node.chosen))?;
        }
        bincode::serialize_into(&mut w, &(self.pending.len() as u64))?;
        for entry in &self.pending {
            bincode::serialize_into(&mut w, entry)?;
        }
        bincode::serialize_into(&mut w, &(self.visited, self.implicit))?;
        w.flush()?;
        w.get_ref().sync_all()?;
        fs::rename(tmp, path)?;
        File::open(
            path.parent()
                .context("certificate work needs a parent directory")?,
        )?
        .sync_all()?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut r = BufReader::new(File::open(path)?);
        let mut magic = [0; 8];
        r.read_exact(&mut magic)?;
        ensure!(&magic == MAGIC, "unsupported certificate work format");
        let header: Header = bincode::deserialize_from(&mut r)?;
        let depth = header
            .forced_depth
            .context("certificate work must have a positive candidate bound")?;
        ensure!(depth > 0 && depth <= 512, "invalid certificate work bound");
        let len: u64 = bincode::deserialize_from(&mut r)?;
        ensure!(len <= MAX_NODES as u64, "certificate work too large");
        let mut nodes = HashMap::with_capacity(len as usize);
        let valid_state = |state| {
            state > 0
                && state <= ((1 << 58) | (3u64.pow(36) - 1)) + 1
                && decode(state).0 < 3u64.pow(36)
        };
        for _ in 0..len {
            let (state, rank, chosen): (u64, u16, u8) = bincode::deserialize_from(&mut r)?;
            ensure!(
                valid_state(state)
                    && rank > 0
                    && rank <= depth
                    && (chosen < 36 || chosen == ALL_MOVES),
                "invalid certificate work node"
            );
            ensure!(
                nodes.insert(state, Node { rank, chosen }).is_none(),
                "duplicate certificate work node"
            );
        }
        let len: u64 = bincode::deserialize_from(&mut r)?;
        ensure!(
            len <= MAX_PENDING as u64,
            "certificate work frontier too large"
        );
        let mut pending = Vec::with_capacity(len as usize);
        for _ in 0..len {
            let (state, rank): (u64, u16) = bincode::deserialize_from(&mut r)?;
            ensure!(
                valid_state(state) && rank > 0 && rank <= depth,
                "invalid certificate work frontier"
            );
            pending.push((state, rank));
        }
        let (visited, implicit): (u64, u64) = bincode::deserialize_from(&mut r)?;
        let mut trailing = [0];
        ensure!(
            r.read(&mut trailing)? == 0,
            "trailing certificate work data"
        );
        Ok(Self {
            certificate: Certificate { header, nodes },
            pending,
            visited,
            implicit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forcing::Objective;
    use crate::rules::Rules;

    #[test]
    fn unfinished_work_roundtrips_without_becoming_a_certificate() {
        let dir = std::env::temp_dir().join(format!("gekitai-work-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("candidate.certificate-work");
        let work = Work::new(Header {
            rules: Rules::default(),
            objective: Objective::Clean,
            first: true,
            start_key: 0,
            forced_depth: Some(25),
        });
        work.save(&path).unwrap();
        let loaded = Work::load(&path).unwrap();
        assert_eq!(loaded.certificate.header, work.certificate.header);
        assert_eq!(loaded.pending, work.pending);
        assert!(loaded.certificate.verify().is_err());
        assert!(Certificate::load(&path).is_err());
        let f = File::options().write(true).open(&path).unwrap();
        f.set_len(f.metadata().unwrap().len() - 1).unwrap();
        assert!(Work::load(&path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
