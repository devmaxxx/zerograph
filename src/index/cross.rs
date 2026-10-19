//! A cross-encoder over the fused candidate list: the question and each candidate's text go
//! through one model together and come out as a relevance score. `ask --rerank` sends that
//! pool to a model command (≈19k input tokens and ~4 s a question, 14/14 on the old paraphrase
//! set); this runs a local model at zero tokens. Opt-in, off every floor until it is measured
//! against them, and loaded through the same `ort` + `tokenizers` path as the embedder.
use anyhow::{anyhow, Context, Result};
use ort::session::Session;
use ort::value::Tensor;
use std::path::{Path, PathBuf};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

/// Ours, not the model's — `max_position_embeddings` is 8194. A question plus the 120-character
/// snippet `rerank::text` builds is well under it, so it only bounds a pathological label.
const MAX_TOKENS: usize = 512;
const BATCH: usize = 16;
/// How many ids the reranker hands back, best first — `ask` shows five seeds.
pub const PICK: usize = 5;

pub struct CrossEncoder { session: Session, tokenizer: Tokenizer, wants_type_ids: bool }

/// Where `optimum-cli export onnx` was told to write: `~/.cache/zerograph/reranker`, beside the
/// embedder's own cache, unless the config names another directory.
pub fn default_dir() -> Result<PathBuf> {
    crate::index::cache_dir("reranker")
}

/// The pad token and its id as the model's own files declare them, the way the embedder reads
/// them from the hub. Both are required: a default here would be a guess at the very thing the
/// files exist to state, and the wrong guess is silent — the attention mask hides it.
fn pad(dir: &Path) -> Result<(String, u32)> {
    let read = |f: &str| -> Result<serde_json::Value> {
        let p = dir.join(f);
        let bytes = std::fs::read(&p).with_context(|| format!("read {}", p.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", p.display()))
    };
    let config = read("config.json")?;
    let tok_config = read("tokenizer_config.json")?;
    let token = tok_config["pad_token"].as_str()
        .with_context(|| format!("{}: pad_token must be a string naming the model's pad token", dir.join("tokenizer_config.json").display()))?
        .to_string();
    let id = config["pad_token_id"].as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .with_context(|| format!("{}: pad_token_id must be the pad token's integer id", dir.join("config.json").display()))?;
    Ok((token, id))
}

/// The exporter's `text-classification` head is `[batch, labels]`, and only a single-label head
/// gives one score per pair. `reranker_dir` is user-configurable, so nothing guarantees the
/// export was that one: a two-label head would interleave two logits per candidate and `pick`
/// would read every second one as its neighbour's score, silently.
fn one_logit_per_row(shape: &[i64], batch: usize) -> Result<()> {
    let labels: i64 = shape.iter().skip(1).product();
    if shape.first().copied() != Some(batch as i64) || labels != 1 {
        anyhow::bail!("reranker logits are {shape:?}, expected [{batch}, 1] — this export has {labels} labels per pair, and only a single-label head can be ranked");
    }
    Ok(())
}

impl CrossEncoder {
    /// `dir` holds `model.onnx`, `tokenizer.json`, `config.json` and `tokenizer_config.json` as
    /// the exporter writes them.
    pub fn open(dir: &Path, threads: usize) -> Result<CrossEncoder> {
        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| anyhow!("{e}")).with_context(|| format!("open reranker tokenizer in {}", dir.display()))?;
        tokenizer.with_truncation(Some(TruncationParams { max_length: MAX_TOKENS, ..Default::default() })).map_err(|e| anyhow!("{e}"))?;
        // The default pad id is 0, which is `<s>` for this model family and `<pad>` for others.
        // The attention mask makes the difference invisible on a graph that honours it — and
        // that is the assumption worth not making, so take the id the model itself declares.
        let (pad_token, pad_id) = pad(dir)?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            pad_token,
            pad_id,
            ..Default::default()
        }));
        // Packed like every reader's session: a reranker answers a person, and the layout it
        // pays for once is read by 200 pairs of GEMMs in the query that opened it.
        let session = crate::index::embed::session_builder(threads, crate::index::embed::Weights::Packed)?
            .commit_from_file(dir.join("model.onnx")).map_err(|e| anyhow!("{e}"))
            .with_context(|| format!("open reranker model in {}", dir.display()))?;
        let wants_type_ids = session.inputs().iter().any(|i| i.name() == "token_type_ids");
        Ok(CrossEncoder { session, tokenizer, wants_type_ids })
    }

    /// One score per text, higher is closer: the model's single logit, unnormalised, because
    /// only the order is used.
    pub fn score(&mut self, question: &str, texts: &[String]) -> Result<Vec<f32>> {
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(BATCH) {
            let pairs: Vec<(String, String)> = chunk.iter().map(|t| (question.to_string(), t.clone())).collect();
            let encodings = self.tokenizer.encode_batch(pairs, true).map_err(|e| anyhow!("{e}"))?;
            let batch = encodings.len();
            let len = encodings.first().map_or(0, |e| e.len());
            let mut ids = Vec::with_capacity(batch * len);
            let mut mask = Vec::with_capacity(batch * len);
            let mut types = Vec::with_capacity(batch * len);
            for e in &encodings {
                ids.extend(e.get_ids().iter().map(|&x| x as i64));
                mask.extend(e.get_attention_mask().iter().map(|&x| x as i64));
                types.extend(e.get_type_ids().iter().map(|&x| x as i64));
            }
            let mut inputs = ort::inputs![
                "input_ids" => Tensor::from_array(([batch, len], ids))?,
                "attention_mask" => Tensor::from_array(([batch, len], mask))?,
            ];
            if self.wants_type_ids {
                inputs.push(("token_type_ids".into(), Tensor::from_array(([batch, len], types))?.into()));
            }
            let outputs = self.session.run(inputs)?;
            let logits = outputs.get("logits")
                .or_else(|| (outputs.len() == 1).then(|| &outputs[0]))
                .context("reranker has no logits output")?;
            let (shape, data) = logits.try_extract_tensor::<f32>()?;
            one_logit_per_row(shape, batch)?;
            out.extend(data.iter().copied());
        }
        Ok(out)
    }
}

/// The ids of the `k` best-scoring candidates, best first; a tie keeps the fused order.
pub fn pick(scores: &[f32], candidates: &[(String, String)], k: usize) -> Vec<String> {
    let mut order: Vec<usize> = (0..candidates.len().min(scores.len())).collect();
    order.sort_by(|&a, &b| scores[b].partial_cmp(&scores[a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
    order.into_iter().take(k).map(|i| candidates[i].0.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cands() -> Vec<(String, String)> {
        vec![("a".to_string(), String::new()), ("b".to_string(), String::new()), ("c".to_string(), String::new())]
    }

    #[test]
    fn pick_orders_by_score_and_breaks_a_tie_by_fused_order() {
        assert_eq!(pick(&[0.1, 0.9, 0.9], &cands(), 5), vec!["b", "c", "a"]);
        assert_eq!(pick(&[0.1, 0.9, 0.9], &cands(), 1), vec!["b"]);
    }

    #[test]
    fn pick_survives_fewer_scores_than_candidates_and_none_at_all() {
        assert_eq!(pick(&[0.5], &cands(), 5), vec!["a"]);
        assert!(pick(&[], &cands(), 5).is_empty());
    }

    #[test]
    fn a_nan_score_sorts_as_a_tie_not_a_panic() {
        assert_eq!(pick(&[f32::NAN, 1.0, 0.0], &cands(), 3).len(), 3);
    }

    #[test]
    fn a_single_label_head_passes_whether_or_not_the_label_axis_is_kept() {
        assert!(one_logit_per_row(&[3, 1], 3).is_ok());
        assert!(one_logit_per_row(&[3], 3).is_ok());
        assert!(one_logit_per_row(&[3, 1, 1], 3).is_ok());
    }

    #[test]
    fn a_two_label_head_is_refused_by_name_rather_than_misread() {
        let e = one_logit_per_row(&[3, 2], 3).unwrap_err().to_string();
        assert!(e.contains("[3, 2]") && e.contains("2 labels"), "{e}");
    }

    #[test]
    fn a_row_count_that_is_not_the_batch_is_refused() {
        assert!(one_logit_per_row(&[2, 1], 3).is_err());
        assert!(one_logit_per_row(&[], 3).is_err());
    }

    #[test]
    #[ignore = "needs the exported model: set ZEROGRAPH_RERANKER_DIR"]
    fn the_model_prefers_the_passage_that_answers_the_question() {
        let dir = crate::env("RERANKER_DIR").expect("ZEROGRAPH_RERANKER_DIR");
        let mut m = CrossEncoder::open(Path::new(&dir), crate::index::embed::threads(crate::index::embed::Resources::default())).unwrap();
        let s = m.score("how is a client's phone number stored", &[
            "Phone numbers are normalised to E.164 before they are stored.".into(),
            "The calendar grid uses a five-minute lattice.".into(),
        ]).unwrap();
        assert!(s[0] > s[1], "{s:?}");
    }
}
