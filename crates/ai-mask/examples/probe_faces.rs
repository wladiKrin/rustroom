//! Spike (M10.18c): can tract run OpenCV Zoo's YuNet and SFace? Prints each output's shape.
//! `cargo run --release --example probe_faces -- <yunet.onnx> <sface.onnx>`
use tract_onnx::prelude::*;

fn run(path: &str, shape: &[usize]) -> TractResult<()> {
    let t = std::time::Instant::now();
    let model = tract_onnx::onnx()
        .model_for_path(path)?
        .with_input_fact(0, f32::fact(shape).into())?
        .into_optimized()?
        .into_runnable()?;
    let load = t.elapsed();
    let input: Tensor = tract_ndarray::ArrayD::<f32>::zeros(shape.to_vec()).into();
    let t = std::time::Instant::now();
    let out = model.run(tvec!(input.into()))?;
    println!("{path}: loaded in {load:?}, ran in {:?}", t.elapsed());
    for (i, o) in out.iter().enumerate() {
        println!("  out {i}: {:?}", o.shape());
    }
    Ok(())
}

fn main() -> TractResult<()> {
    let a: Vec<String> = std::env::args().collect();
    run(&a[1], &[1, 3, 640, 640])?;
    run(&a[2], &[1, 3, 112, 112])?;
    Ok(())
}
