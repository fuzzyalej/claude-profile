use std::path::Path;

// Bakes profiles/ and plugins/coordinator/ into the binary so installed builds can seed
// them at runtime; the source tree is not available once the binary ships.
fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = std::env::var("OUT_DIR").unwrap();
    bake_profiles(Path::new(&manifest), Path::new(&out_dir));
    bake_plugin(Path::new(&manifest), Path::new(&out_dir));
}

fn bake_profiles(manifest: &Path, out_dir: &Path) {
    let src = manifest.join("profiles");
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

    std::fs::write(out_dir.join("bundled_files.rs"), out).unwrap();
}

fn bake_plugin(manifest: &Path, out_dir: &Path) {
    let src = manifest.join("plugins").join("coordinator");
    let mut files = Vec::new();
    collect(&src, &src, &mut files);
    files.sort();

    let mut out = String::from("pub static BUNDLED_PLUGIN_FILES: &[(&str, &str)] = &[\n");
    for (rel, path) in &files {
        out.push_str(&format!("    ({:?}, include_str!({:?})),\n", rel, path));
    }
    out.push_str("];\n");

    std::fs::write(out_dir.join("bundled_plugin_files.rs"), out).unwrap();
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, String)>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let entries = std::fs::read_dir(dir).expect("plugins/coordinator/ must exist");
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            files.push((rel, path.display().to_string()));
        }
    }
}
