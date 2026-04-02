fn main() {
    // アイコンリソースをコンパイル（.rcファイル経由で複数アイコンを登録）
    let mut res = winresource::WindowsResource::new();
    res.set_resource_file("resources/gv.rc");

    // Cargo.tomlのバージョンからVERSIONINFO設定
    let version = env!("CARGO_PKG_VERSION");
    let parts: Vec<u64> = version.split('.').filter_map(|s| s.parse().ok()).collect();
    let major = parts.first().copied().unwrap_or(0);
    let minor = parts.get(1).copied().unwrap_or(0);
    let patch = parts.get(2).copied().unwrap_or(0);
    let ver_u64 = major << 48 | minor << 32 | patch << 16;

    res.set_version_info(winresource::VersionInfo::FILEVERSION, ver_u64);
    res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, ver_u64);
    res.set("FileDescription", "ぐらびゅ");
    res.set("ProductName", "ぐらびゅ");
    res.set("FileVersion", version);
    res.set("ProductVersion", version);
    res.set("LegalCopyright", "MIT License");

    if let Err(e) = res.compile() {
        eprintln!("リソースコンパイル警告: {e}");
    }
}
