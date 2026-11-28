//! Embeds Horizon's icon (`packaging/icons/horizon.ico`) in the Windows executables, as icon
//! resource 1: Explorer and the task bar show it, and the window takes it as its icon.

const SEGMENT_HEAP_ENV: &str = "HORIZON_WINDOWS_SEGMENT_HEAP";

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let ico = format!("{dir}/../../packaging/icons/horizon.ico");
    println!("cargo:rerun-if-changed={ico}");
    println!("cargo:rerun-if-env-changed={SEGMENT_HEAP_ENV}");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(&ico);
        if let Err(e) = resource.compile() {
            panic!("embedding {ico}: {e}");
        }
        if std::env::var(SEGMENT_HEAP_ENV).as_deref() == Ok("1") {
            embed_segment_heap_manifest();
        }
    }
}

#[cfg(windows)]
fn embed_segment_heap_manifest() {
    const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"
          xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
  <assemblyIdentity type="win32" name="Horizon.Client" version="1.0.0.0" />
  <asmv3:application>
    <asmv3:windowsSettings xmlns="http://schemas.microsoft.com/SMI/2020/WindowsSettings">
      <heapType>SegmentHeap</heapType>
    </asmv3:windowsSettings>
  </asmv3:application>
</assembly>
"#;

    assert!(
        std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc"),
        "{SEGMENT_HEAP_ENV}=1 requires a Windows MSVC target"
    );
    let out_dir = std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR");
    let manifest = std::path::Path::new(&out_dir).join("horizon-segment-heap.manifest");
    std::fs::write(&manifest, MANIFEST).expect("write Segment Heap manifest");

    // Merge with the linker manifest so the Windows icon and other binaries
    // still use their existing resource build.
    println!("cargo:rustc-link-arg-bin=horizon-client=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=horizon-client=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
