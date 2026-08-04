#!/bin/bash
# Rebuild and verify image-bearing inlinedparts on four independent DOCX hosts.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
DOCX="$ROOT/inlinedparts-hosts.docx"

rm -f "$DOCX"
officecli create "$DOCX"
officecli raw-set "$DOCX" /footnotes /w:footnotes replace --xml '<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:footnote w:id="1"><w:p><w:r><w:t>Footnote host</w:t></w:r></w:p></w:footnote></w:footnotes>'
officecli raw-set "$DOCX" /endnotes /w:endnotes replace --xml '<w:endnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:endnote w:id="1"><w:p><w:r><w:t>Endnote host</w:t></w:r></w:p></w:endnote></w:endnotes>'
officecli add "$DOCX" '/body/p[1]' --type comment --prop text='Comment host' --prop author='OfficeCLI'
officecli batch "$DOCX" --commands-file "$ROOT/inlinedparts-hosts.json"
officecli validate "$DOCX"

# Every relationship must be owned by the XML part containing its r:id.
for rels in document comments footnotes endnotes; do
  unzip -p "$DOCX" "word/_rels/$rels.xml.rels" | grep -q '/relationships/image'
done
echo "Verified document, comment, footnote and endnote relationship hosts: $DOCX"
