# El canal del CI Health deja de reportar los fallos reales — Tasks

- [x] T1 · S1, S2 — `.github/workflows/ci-health.yml`: el clasificador matchea una **línea de fallo**, no una mención (`FAILURE_LINE_RE` en el step "Classify Bug Discovery result").
- [x] T2 · S1, S2, S5 — harness del observer: caso nuevo RED contra el clasificador roto + GREEN con el fix (secciones 11 y 12 de `scripts/test_ci_health_close.sh`).
- [x] T3 · S3 — el cierre del observer matchea el subtítulo `"CI failed on main"` (`CI_CLOSE_KEY`): cierra por origen y por título retirado, con una sola implementación (`close_mine`).
- [x] T4 · S3, S4 — #2162 y #1980 cerrados con la evidencia; #1936 comentado con el estado real y queda abierto.
- [x] T5 · S1–S5 — verificación: harnesses verdes, `bash -n`, y el RED demostrado (104 checks en verde; RED observado en 2 + 4 checks).

Spec: `odd/tasks/ci-health-w03-classifier.md`. Cierra #2233, #2162, #1980.
