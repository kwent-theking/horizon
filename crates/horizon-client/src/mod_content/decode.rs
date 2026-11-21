//! A cold build's mod files, decoded on several threads but never more than
//! [`BUDGET`] bytes at once, and handed over in mod and file order.
use super::*;
use horizon_mods::content::{FileReader, MAX_FILE};

/// Decoded bytes a cold build holds at once beside its tables. Decoding whole
/// mods per thread (ATM10 carries some of hundreds of MB) passed the 3 GB peak.
pub(super) const BUDGET: u64 = 256 << 20;

/// One worker's share of a batch: files `.1` of mod `.0`.
pub(super) type Slice = (usize, Range<usize>);

/// The batches to decode `mods` in, one after another.
pub(super) fn batches(mods: &[ContentMod], threads: usize) -> Vec<Vec<Slice>> {
    let sizes = horizon_core::par::par_map_with(threads, mods, |m| file_sizes(&m.content));
    plan(&sizes, BUDGET, threads)
}

/// Each file's size before decoding it, from the archive's directory or the
/// folder. One that can't be told counts as the largest allowed: its read
/// reports why, in order.
fn file_sizes(content: &Content) -> Vec<u64> {
    let Ok(mut reader) = content.reader() else {
        return vec![MAX_FILE; content.files.len()];
    };
    content
        .files
        .iter()
        .map(|file| {
            let size = match &mut reader {
                FileReader::Dir(root) => fs::metadata(root.join(file)).ok().map(|m| m.len()),
                // Raw: the header is read, nothing is inflated.
                FileReader::Zip(zip) => zip
                    .index_for_name(file)
                    .and_then(|i| zip.by_index_raw(i).ok().map(|f| f.size())),
            };
            size.unwrap_or(MAX_FILE).min(MAX_FILE + 1)
        })
        .collect()
}

/// Cuts the files (`sizes[mod][file]`, in order) into batches of at most
/// `budget` bytes (a bigger file alone), each split into slices of about a
/// thread's share that never span two mods. Their concatenation is every file
/// in order.
pub(super) fn plan(sizes: &[Vec<u64>], budget: u64, threads: usize) -> Vec<Vec<Slice>> {
    let mut batches = Vec::new();
    let mut batch: Vec<(usize, usize, u64)> = Vec::new();
    let mut held = 0u64;
    for (m, files) in sizes.iter().enumerate() {
        for (f, &size) in files.iter().enumerate() {
            if !batch.is_empty() && held.saturating_add(size) > budget {
                batches.push(slices(&batch, held, threads));
                batch.clear();
                held = 0;
            }
            batch.push((m, f, size));
            held = held.saturating_add(size);
        }
    }
    if !batch.is_empty() {
        batches.push(slices(&batch, held, threads));
    }
    batches
}

fn slices(batch: &[(usize, usize, u64)], held: u64, threads: usize) -> Vec<Slice> {
    let share = held.div_ceil(threads.max(1) as u64).max(1);
    let mut out: Vec<Slice> = Vec::new();
    let mut bytes = 0u64;
    for &(m, f, size) in batch {
        match out.last_mut() {
            Some((last, files)) if *last == m && files.end == f && bytes < share => {
                files.end = f + 1;
                bytes = bytes.saturating_add(size);
            }
            _ => {
                out.push((m, f..f + 1));
                bytes = size;
            }
        }
    }
    out
}

/// The bytes of one slice's files, in order.
pub(super) fn read(
    mods: &[ContentMod],
    (m, files): &Slice,
) -> std::result::Result<Vec<Vec<u8>>, String> {
    let content = &mods[*m].content;
    let mut reader = content.reader()?;
    content.files[files.clone()]
        .iter()
        .map(|file| reader.read(file))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flatten(batches: &[Vec<Slice>]) -> Vec<(usize, usize)> {
        batches
            .iter()
            .flatten()
            .flat_map(|(m, files)| files.clone().map(move |f| (*m, f)))
            .collect()
    }

    #[test]
    fn plan_keeps_every_file_in_order_within_the_budget() {
        let sizes = vec![
            vec![10, 10, 10, 10, 10],
            vec![],
            vec![40],
            vec![5; 9],
            vec![200, 1],
        ];
        let budget = 50;
        let batches = plan(&sizes, budget, 3);
        let expected: Vec<(usize, usize)> = sizes
            .iter()
            .enumerate()
            .flat_map(|(m, files)| (0..files.len()).map(move |f| (m, f)))
            .collect();
        assert_eq!(flatten(&batches), expected);
        for batch in &batches {
            assert!(!batch.is_empty());
            let files: Vec<(usize, usize)> = flatten(std::slice::from_ref(batch));
            let held: u64 = files.iter().map(|&(m, f)| sizes[m][f]).sum();
            assert!(held <= budget || files.len() == 1, "{batch:?} holds {held}");
            for (m, files) in batch {
                assert!(!files.is_empty() && files.end <= sizes[*m].len());
            }
        }
        // The oversized file is a batch of its own.
        assert!(batches.contains(&vec![(4, 0..1)]));
    }

    #[test]
    fn plan_splits_one_big_mod_across_threads() {
        let sizes = vec![vec![1; 12]];
        let batches = plan(&sizes, 100, 4);
        assert_eq!(batches, [vec![(0, 0..3), (0, 3..6), (0, 6..9), (0, 9..12)]]);
        assert!(plan(&[], 100, 4).is_empty());
        assert!(plan(&[vec![], vec![]], 100, 4).is_empty());
        assert_eq!(plan(&[vec![0, 0]], 100, 0), [vec![(0, 0..2)]]);
    }
}
