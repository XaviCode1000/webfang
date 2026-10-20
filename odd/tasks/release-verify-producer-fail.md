# Release verification producer-fail aware (Ola 1, Part B audit #2128)

Fixوية del audit scripts-fail-open 2026-10-18: los checkpoints del pipeline de release emiten su estado
de fallback legítimo ("verificado", "already complete", "OK") también cuando el productor falló. Ola 1
mata B5, B7, B8 (+ PRs-B/C del mismo child treed in #2128 follow this PR sequentially base main).

## Specs

- S1: **B7** — "verifying the release assets" steps must derive the expected set from the EXPECTED
  artifact names and fail when ANY expected asset is missing (current: `ls -1` derives a mapfile map the
  check survivors, missing `webfang.spdx.json` alone passes rc 0). Sev HIGH.
- S2: **B8** — "Release $TAG is already complete; nothing to do" (ensure-release.sh:96) must require
  the full attestation set (spdx + notices + SHA256SUMS) and non-zero sizes; today 5 base names with
  zero-size dist caraves rc 0.
- S3: **B5** — provenance tag/manifest suffix comping must be grammatically symmetric: today a tag
  RC (`v3.1.0-rc.1`) REJECTS the correct RC manifest (`::error::L1.2` — false reject) and ACCEPTS a
  tag RC against a stable manifest `3.1.0` ("L1 preflight passed", rc 0 — false accept).
- S4: regresión obligatoria: B1–B4 keep passing (los cerrados por #2060/#1668 no se rompen).

## Tasks

- T1 (S1) PR-A slice 1: release.yml, los N pasos de verificación de assets: checklist derivada de la
  lista esperada, fail nombrando el asset faltante. commit fix(release).
- T2 (S2) PR-A slice 2: ensure-release.sh "already complete" exige spdx + notices + SHA256 + tamaños>0.
  commit fix(release) + tests del harness del guard.
- T3 (S3) PR-A slice 3: check_release_provenance.sh strip simétrico -rc.N (o rechazo nombrado de
  divergencia de gramáticas tag-RC/manifest-stable). commit fix(release).
- T4 (S4) verificación final release_repro.sh casos B5/B7/B8 defect=0 y B1–B4 pasando; PR `Closes part of #2128`.
- T5 PR-B (#2128) y PR-C (#2128) siguen SECUENCIALES sobre main — fuera de este feature doc (slices posteriores).

## Log

- L1: user pidió plan global y umbrella (issues #2127–#2133); dijo "adelante, todas las issues ya
  tienen label approved" — verificado #2128 status:approved (2026-10-20).
- L2: arranque del slice T1–T4 en worktree fix-release-verify-producer-fail, branch
  fix/release-verify-producer-fail desde main 2865dab8.
- L3: T1 (B7) resuelto en .github/workflows/release.yml: env RELEASE_EXPECTED_ASSETS con los 7 nombres
  canónicos y prechecks fail-closed en sign/verify/publish (smoke: falta un asset → exit 1 nombrándolo;
  inventario completo → exit 0). RED canónico: corrida 9 del repro (run1/run2.log, B7 confirmed).

- L4: T2 (B8) commit 4a4a9092: ensure-release.sh completa la inventería (7 nombres + bundle+size>0, JSON crudo
  con jq, falla-cerrada no-parsable); harnesses 35/24 verde; fixture shapes ahora {name,size}. El worker
  (subagent) terminó el harness squash (verde 24/0, 35/0).
- L5: T3 (B5) commit a8b6d7fc: L1.2 compara semver completo sin strip; I.10 corregido + I.10b/I.10c;
  test_release_provenance 75/75.
- L6: T1 re-verified en vivo tras armar el drive real de release.yml: (a) el caso B7 del repro está
  anclado verbatim al cuerpo PED-DE-fix (no puede ver el fix) — la verificación viva se hizo extrayendo
  el paso real con pyYAML y stub de cosign/parse GH: sign 6/7 → rc 1 nombrando al faltante, 7/7 → rc 0
  'signed 7 assets'; publish precheck 1/7 → rc 1 lista nombrada, 7/7 → INVENTORY-OK. (b) el drive descubró
  un falso-positivo propio: el heredoc añade un @NL@ final que produce la línea vacía y una rechazo del
  inventario completo — corrigado con tail-safety guard + if explícito en los 3 sitios (re-ampliado al
  commit T1 3b697812).
- L7: T4: ci_fast_gate.sh GREEN (7 PASS /10 SKIP, actionlint ok); release_repro 2 corridas idénticas:
  6 matched (B1,B2,B3,B4,B5,B8 defect=0) + 9 confirmados (B6,B7,B9-B15 — fuera de alcance de este PR;
  B7 verificado en vivo por drive). PR: Closes part of #2128+ (PR-A de la Ola 1; B6→PR-B y B10→PR-C siguen).
