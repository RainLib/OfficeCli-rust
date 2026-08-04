# Relationship-owning DOCX inline hosts

Run `bash inlinedparts-hosts.sh` to create `inlinedparts-hosts.docx`.

The demo inserts the same VML image carrier into document text, a comment,
a footnote, and an endnote. It validates the package and checks that each
host has its own `word/_rels/<host>.xml.rels` image relationship, rather than
incorrectly attaching every resource to `word/document.xml`.

The small PNG payload is intentionally synthetic: the demo verifies OOXML
relationship ownership and round-trip wiring, not image rendering quality.
