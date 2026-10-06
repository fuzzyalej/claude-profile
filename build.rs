use std::path::Path;

// Bakes profiles/ into the binary so installed builds can seed them at runtime; the
// source tree is not available once the binary ships.
fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let src = Path::new(&manifest).join("profiles");
    println!("cargo:rerun-if-changed={}", src.display());

    let mut names: Vec<String> = std::fs::read_dir(&src)
        .expect("profiles/ must exist")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".json") || n.ends_with(".lock"))
        .collect();
    names.sort();

    let mut out = String::from("pub static BUNDLED_FILES: &[(&str, &str)] = &[\n");
    for name in &names {
        let path = src.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        out.push_str(&format!("    ({:?}, include_str!({:?})),\n", name, path.display().to_string()));
    }
    out.push_str("];\n");

    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("bundled_files.rs");
    std::fs::write(dest, out).unwrap();
}
