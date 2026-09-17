//! Batched inference smoke probe (issue #1456, P0-001 batch prototype).
//!
//! Proves a real `(N, S_max)` batched `Session::run` works end-to-end through
//! ort 2.0.0-rc.12 against the REAL cached Granite-97M model (read-only, never
//! downloaded) and that per-row embeddings match single-chunk runs
//! (CORRECTNESS gate first: CORRECTNESS > ROBUSTNESS > PREDICTABILITY >
//! PERFORMANCE).
//!
//! - PARITY: batched (N=4, mixed seq lens) vs 4 single `(1, S)` runs, asserting
//!   per-row closeness. Tolerance 1e-5: both paths run identical weights and
//!   identical per-row summation order over masked tokens, so any diff is pure
//!   float rounding on unit-scale vectors — 1e-5 is conservative slack.
//! - NUMBERS: wall time + peak RSS (`VmHWM` from `/proc/self/status`, the only
//!   portable-here source; no repo RSS helper exists) for batch sizes
//!   {1,2,4,8,16} over identical synthetic chunks, printed as a table.
//!
//! Skips gracefully (pass, with a clear message) when the cached model file is
//! absent — this probe never downloads.
//!
//! Requires the `ai` feature. Single session + `intra_threads(1)` unchanged:
//! this probe isolates batching, not the pool.

#![cfg(feature = "ai")]

use std::time::Instant;

use ort::session::Session;
use webfang_ai::infrastructure_ai::embedding_ops::{l2_normalize_safe, mean_pool};
use webfang_ai::infrastructure_ai::inference_engine::{
    run_batched_inference, InputPlan, ModelInput,
};
use webfang_ai::infrastructure_ai::AiModel;

#[path = "p0_001_common.rs"]
#[allow(dead_code)]
mod p0_001_common;

/// Per-row closeness gate: identical weights + identical per-row summation
/// order ⇒ only float-rounding diffs on unit-scale vectors; 1e-5 is slack.
const PARITY_TOLERANCE: f32 = 1e-5;

/// Batch sizes for the NUMBERS sweep (identical synthetic chunks each).
const BATCH_SIZES: [usize; 5] = [1, 2, 4, 8, 16];

/// Deterministic synthetic chunk: `[CLS] content… [SEP]` with small in-vocab
/// ids (safe for any BERT-style vocab) and a dense mask.
fn synthetic_input(seq_len: usize, seed: u64) -> ModelInput {
    assert!(seq_len >= 2, "synthetic chunks need room for specials");
    let mut ids = Vec::with_capacity(seq_len);
    ids.push(101);
    for i in 1..seq_len - 1 {
        ids.push(200 + ((i as u64 * 37 + seed * 11) % 4000) as i64);
    }
    ids.push(102);
    ModelInput::from_tokens(ids)
}

/// Single-chunk reference: mirrors the production single path (`(1, S)`
/// tensors → run → `mean_pool` → Matryoshka take → L2) without touching it.
fn run_single_reference(
    session: &mut Session,
    plan: &InputPlan,
    input: &ModelInput,
    variant: AiModel,
) -> Vec<f32> {
    let seq_len = input.seq_len();
    let mut named: Vec<(
        std::borrow::Cow<'_, str>,
        ort::session::SessionInputValue<'_>,
    )> = Vec::with_capacity(plan.names().len());
    for name in plan.names() {
        let flat: Vec<i64> = match name.as_str() {
            "input_ids" => input.input_ids.clone(),
            "attention_mask" => input.attention_mask.clone(),
            "token_type_ids" => input.token_type_ids.clone(),
            other => panic!("plan de prueba inesperado: {other}"),
        };
        let array = ndarray::Array2::<i64>::from_shape_vec((1, seq_len), flat)
            .expect("el tensor single (1, S) debe construirse");
        let tensor = ort::value::Tensor::from_array(array).expect("el tensor ORT debe crearse");
        named.push((std::borrow::Cow::Borrowed(name.as_str()), tensor.into()));
    }
    let outputs = session
        .run(named)
        .expect("la ejecución single debe funcionar");
    let (_, raw): (_, &[f32]) = outputs["last_hidden_state"]
        .try_extract_tensor::<f32>()
        .expect("last_hidden_state debe extraerse");
    let pooled = mean_pool(raw, seq_len, variant.embedding_dim(), &input.attention_mask);
    let truncated: Vec<f32> = pooled.iter().take(variant.output_dim()).copied().collect();
    l2_normalize_safe(&truncated)
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}

#[test]
fn batched_parity_vs_single_runs() {
    let Some(setup) = p0_001_common::setup_probe("batched_parity_vs_single_runs") else {
        return;
    };
    let variant = setup.variant;
    let mut session = setup.session;
    let plan = setup.plan;

    // N=4 with mixed sequence lengths (the padding path under test).
    let seq_lens = [7usize, 13, 5, 11];
    let inputs: Vec<ModelInput> = seq_lens
        .iter()
        .enumerate()
        .map(|(i, s)| synthetic_input(*s, i as u64 + 1))
        .collect();

    let singles: Vec<Vec<f32>> = inputs
        .iter()
        .map(|input| run_single_reference(&mut session, &plan, input, variant))
        .collect();
    let batched = run_batched_inference(&mut session, &plan, &inputs, variant)
        .expect("la inferencia batch debe funcionar");

    assert_eq!(
        batched.len(),
        inputs.len(),
        "el orden y conteo de salida deben coincidir con la entrada"
    );
    let mut worst = 0.0f32;
    for (row, (single, batch)) in singles.iter().zip(batched.iter()).enumerate() {
        assert_eq!(batch.len(), 384, "cada fila debe ser 384d");
        let diff = max_abs_diff(single, batch);
        println!(
            "fila {row} (S={}): max abs diff = {diff:.3e}",
            seq_lens[row]
        );
        worst = worst.max(diff);
        assert!(
            diff < PARITY_TOLERANCE,
            "fila {row}: diff {diff:.3e} supera la tolerancia {PARITY_TOLERANCE:.0e}"
        );
    }
    println!("PARITY OK: peor max abs diff = {worst:.3e} (tolerancia {PARITY_TOLERANCE:.0e})");
}

#[test]
fn batched_timing_and_rss_table() {
    let Some(setup) = p0_001_common::setup_probe("batched_timing_and_rss_table") else {
        return;
    };
    let variant = setup.variant;
    let mut session = setup.session;
    let plan = setup.plan;

    // Untimed warm-up so the table measures steady-state runs, not arena init.
    let warmup = vec![synthetic_input(12, 99)];
    run_batched_inference(&mut session, &plan, &warmup, variant)
        .expect("el calentamiento debe funcionar");

    println!("| batch | seq/chunk | tiempo pared | pico RSS (VmHWM) | dim |");
    println!("|------:|----------:|-------------:|-----------------:|-----|");
    for batch in BATCH_SIZES {
        let inputs: Vec<ModelInput> = (0..batch).map(|_| synthetic_input(12, 7)).collect();
        let started = Instant::now();
        let outputs = run_batched_inference(&mut session, &plan, &inputs, variant)
            .expect("la inferencia batch debe funcionar");
        let elapsed = started.elapsed();
        assert_eq!(outputs.len(), batch, "una salida por entrada, en orden");
        assert!(
            outputs.iter().all(|v| v.len() == 384),
            "todas las filas deben ser 384d"
        );
        let rss = p0_001_common::peak_rss_kib()
            .map(|kb| format!("{kb} kB"))
            .unwrap_or_else(|| "n/d".to_string());
        println!(
            "| {batch:>5} | {:>9} | {:>11.3?} | {:>16} | 384 |",
            12, elapsed, rss
        );
    }
    println!(
        "NOTA: VmHWM es marca de agua alta del proceso (monotónica); cada fila muestra el pico acumulado hasta ese punto, no el delta del batch."
    );
}
