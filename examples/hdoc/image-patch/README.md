# HCD image patch example

可直接打开仓库中已经生成的对比页：

- [`output/pictures-basic-20260903/index.html`](output/pictures-basic-20260903/index.html)
- revision 0 HCD：[`02-hcd-original.html`](output/pictures-basic-20260903/02-hcd-original.html)
- revision 1 HCD：[`03-hcd-patched.html`](output/pictures-basic-20260903/03-hcd-patched.html)
- 实际 patch：[`patch-pictures-basic.json`](patch-pictures-basic.json)

这个固定示例以 `examples/ppt/pictures/pictures-basic.pptx` 为输入，共映射 25 个图片节点。
patch 将第一张图片替换为第二张图片的内容寻址资源，并修改第一张图片的矩形；同一个
`nodeId` 在 revision 0 和 revision 1 中保持不变。

This example uses only the in-process Rust HCD pipeline. Start with an existing bundle containing at
least one mapped DOCX/XLSX/PPTX/PDF picture.

```bash
target/debug/officecli hdoc list-images document.hcd --limit 10 --json
target/debug/officecli hdoc get-image document.hcd n_0123456789abcdef0123456789abcdef --json
target/debug/officecli hdoc put-asset document.hcd replacement.png --json
```

Copy the returned `nodeId`, current `visualHash`, and staged `hash` into `patch.json` using the
`hcd-patch/3` example in `docs/hcd-docx-v1.zh.md`, then apply and inspect it:

```bash
target/debug/officecli hdoc apply document.hcd \
  --patch patch.json \
  --expected-revision 0 \
  --json

target/debug/officecli hdoc validate document.hcd --json
target/debug/officecli hdoc render-html document.hcd \
  --revision 1 \
  --output image-revision-1.html \
  --json

# Rebuild with the patched raster using Rust handlers. Do not pass --source yet.
target/debug/officecli hdoc export document.hcd \
  --revision 1 \
  --output image-revision-1.pptx \
  --json
```

`put-asset` is staging only: revision 0 remains unchanged until `image.replace` succeeds. A stale
`visualHash`, an unknown/unverified asset, unsafe geometry, or a concurrent edit fails atomically.
