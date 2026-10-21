# Scripts fail-open — Ola 3 (flip del gate intra-crate) Tasks

- [x] T1 · S1–S4, S6 — el gate de dirección consume el extractor con `syn`; el escáner awk se borra y no queda fallback (PR #2190).
- [x] T2 · S5 — la regla de B44 (una entry del allowlist que matchea cero sitios es un fallo) se arrastra al gate nuevo, tal cual venía de #2174.
- [x] T3 · S1–S6 — el harness del gate pasa de 14 a 36 checks: extractor ausente ⇒ fail-closed, parse error ⇒ fail-closed nombrando el fichero, las seis formas de path, la región de test estructural, el choke, la entry caduca y la ausencia del escáner de texto.
- [x] T4 · S1–S6 — la salida del gate nuevo es byte-idéntica a la del gate awk sobre el árbol real y el canon de la clase queda 11/0.
