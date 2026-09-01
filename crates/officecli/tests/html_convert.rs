use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Read;
use std::path::Path;

fn officecli() -> Command {
    Command::cargo_bin("officecli").unwrap()
}

fn write_html(path: &Path) {
    std::fs::write(
        path,
        r#"<!doctype html>
<html lang="zh-CN">
<head><title>转换测试</title><style>body { color: red }</style></head>
<body>
  <section>
    <h1>智能脱敏服务</h1>
    <p>Secret 123 &amp; 中文内容 <a href="https://example.com">链接</a></p>
    <ul><li>第一项</li><li>第二项</li></ul>
    <table>
      <tr><th>Name</th><th>Value</th></tr>
      <tr><td>Account</td><td>6222</td></tr>
    </table>
  </section>
  <section><h1>第二部分</h1><p>分页内容</p></section>
  <script>document.write('must not appear')</script>
</body>
</html>"#,
    )
    .unwrap();
}

fn convert_and_validate(source: &Path, output: &Path) {
    officecli()
        .args([
            "convert",
            source.to_string_lossy().as_ref(),
            "--output",
            output.to_string_lossy().as_ref(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""engine": "rust-html-semantic""#,
        ))
        .stdout(predicate::str::contains(r#""fidelity": "semantic""#));
    officecli()
        .args(["validate", output.to_string_lossy().as_ref()])
        .assert()
        .success();
}

fn pptx_contains_native_table(path: &Path) -> bool {
    let file = std::fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        if entry.name().starts_with("ppt/slides/slide") && entry.name().ends_with(".xml") {
            let mut xml = String::new();
            entry.read_to_string(&mut xml).unwrap();
            if xml.contains("<a:tbl>") && xml.contains("Account") && xml.contains("6222") {
                return true;
            }
        }
    }
    false
}

#[test]
fn html_always_uses_the_in_process_rust_converter() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.html");
    write_html(&source);

    // Even an explicitly selected external engine cannot change the HTML
    // dispatch path. This guards the early HTML routing in handle_convert.
    for (engine, extension) in [("libreoffice", "docx"), ("pdf2docx", "pdf")] {
        let output = temp.path().join(format!("rust-only-{engine}.{extension}"));
        officecli()
            .args([
                "convert",
                source.to_string_lossy().as_ref(),
                "--output",
                output.to_string_lossy().as_ref(),
                "--engine",
                engine,
                "--json",
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains(
                r#""engine": "rust-html-semantic""#,
            ));
        assert!(output.exists());
    }
}

#[test]
fn html_converts_to_docx_xlsx_pptx_and_pdf() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.html");
    write_html(&source);

    for extension in ["docx", "xlsx", "pptx", "pdf"] {
        let output = temp.path().join(format!("output.{extension}"));
        convert_and_validate(&source, &output);
        officecli()
            .args(["view", output.to_string_lossy().as_ref()])
            .assert()
            .success()
            .stdout(predicate::str::contains("智能脱敏服务"))
            .stdout(predicate::str::contains("Secret 123"))
            .stdout(predicate::str::contains("must not appear").not());
        if extension == "pptx" {
            assert!(pptx_contains_native_table(&output));
        }
    }
}

#[test]
fn html_conversion_requires_an_explicit_supported_target() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.html");
    write_html(&source);

    officecli()
        .args(["convert", source.to_string_lossy().as_ref()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires --output"));

    let output = temp.path().join("output.txt");
    officecli()
        .args([
            "convert",
            source.to_string_lossy().as_ref(),
            "--output",
            output.to_string_lossy().as_ref(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("HTML can convert to"));
    assert!(!output.exists());
}
