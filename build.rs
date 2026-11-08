// SPDX-License-Identifier: GPL-3.0-or-later
fn main() {
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_default_translation_context(slint_build::DefaultTranslationContext::None)
        .with_bundled_translations("lang");
    slint_build::compile_with_config("ui/app.slint", config).expect("failed to compile the Slint UI");
    println!("cargo:rerun-if-changed=lang");
    println!("cargo:rerun-if-changed=ui");

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set_icon_with_id("assets/icon.ico", "2"); // tray icon (same artwork)
        res.set("ProductName", "Solua");
        res.set("FileDescription", "Solua — DDC/CI monitor control");
        res.set("LegalCopyright", "GPL-3.0-or-later");
        let _ = res.compile();
    }
}
