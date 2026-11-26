# Synthetic performance comparison

## Method

The ignored Rust benchmark uses the test-only legacy writer and the real active SQLCipher repository. It compares the same initial data and operations using the same temporary filesystem and debug build. Each size runs five independent fixture pairs. Measurements use monotonic wall-clock elapsed time inside Rust. Preparation, legacy-to-SQLCipher import, and final cleanup are outside timed operations. Each fixture contains General, three named projects, 25/250/1,000 generated mappings distributed across two scopes, and 30 saved history records split between General and one named project. History inputs are about 5 KiB and use synthetic text. The benchmark lists, looks up by mapping UUID, persists a new mapping, reuses it, pseudonymizes another input, retrieves history, clears General and deletes one named project.

The measured machine ran NixOS x86_64, Intel Core i5-1335U, Rust 1.99.0, SQLCipher 4.20.0 and an ext-family temporary filesystem. The table uses the final serial rerun after other tests and build work finished. An earlier concurrent run showed substantial write and unlock variance, particularly at 25 rows. The Linux link-only symbol isolation does not change the repository, cipher settings or benchmark code.

To rerun from `apps/desktop/src-tauri` inside the Nix shell:

```sh
cargo test --locked --offline database::benchmark::compare_legacy_and_sqlcipher -- --ignored --nocapture --test-threads=1
```

## Results

Each cell is median [minimum–maximum] in milliseconds over five samples. The default open uses each format's real passphrase mode, including its own KDF parameters.

| Operation | 25 JSON | 25 SQLCipher | 250 JSON | 250 SQLCipher | 1,000 JSON | 1,000 SQLCipher |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Unlock and open | 388.582 [371.843–402.850] | 1700.775 [1648.882–1838.008] | 396.638 [388.757–415.911] | 1689.622 [1676.030–1810.436] | 426.572 [417.732–479.756] | 1773.493 [1654.231–1907.532] |
| List projects | 0.005 [0.004–0.005] | 0.088 [0.088–0.093] | 0.005 [0.004–0.005] | 0.130 [0.130–0.148] | 0.005 [0.005–0.005] | 0.259 [0.255–0.282] |
| Look up mapping by ID | 0.001 [0.001–0.002] | 0.079 [0.079–0.090] | 0.004 [0.004–0.005] | 0.087 [0.086–0.094] | 0.012 [0.011–0.021] | 0.094 [0.087–0.116] |
| Insert new mapping | 84.516 [78.534–89.813] | 3.532 [2.588–4.257] | 100.755 [95.639–102.680] | 3.301 [2.647–3.987] | 131.993 [129.281–176.004] | 3.337 [2.840–33.541] |
| Save previously mapped value | 81.095 [75.642–82.074] | 0.132 [0.125–0.196] | 93.275 [90.608–101.251] | 0.143 [0.125–0.157] | 126.798 [117.226–133.188] | 0.138 [0.136–0.156] |
| Pseudonymize and save | 83.449 [78.235–92.839] | 3.758 [2.999–4.479] | 99.672 [93.471–101.066] | 3.166 [3.013–3.434] | 132.141 [126.017–143.219] | 3.897 [3.193–4.421] |
| Retrieve history | 0.015 [0.013–0.020] | 0.643 [0.586–0.976] | 0.014 [0.013–0.027] | 0.636 [0.618–1.231] | 0.014 [0.014–0.020] | 0.605 [0.580–1.045] |
| Clear General history | 41.611 [39.736–44.941] | 4.696 [4.268–5.162] | 52.141 [49.145–55.077] | 4.803 [4.590–5.096] | 79.922 [77.729–85.732] | 4.727 [4.628–5.408] |
| Delete named project | 1.756 [1.667–1.823] | 5.015 [4.813–5.352] | 8.911 [8.694–9.775] | 5.832 [5.289–6.098] | 32.125 [30.931–37.667] | 8.270 [8.203–8.858] |

Sizes are representative initial on-disk bytes from the last trial at each size. SQLCipher includes the referenced database and active bootstrap. Both exclude lock files; the retained legacy recovery copy and unreferenced generations are excluded from SQLCipher active size.

| Seed mappings | Encrypted JSON bytes | SQLCipher database + bootstrap bytes |
| ---: | ---: | ---: |
| 25 | 1,124,512 | 467,372 |
| 250 | 1,253,628 | 565,674 |
| 1,000 | 1,682,188 | 848,298 |

## Interpretation

SQLCipher generally reduced durable write times. SQLCipher updates indexed rows and pages, while the prior writer cloned and re-encrypted the full JSON even when saving an already known mapping. The SQLCipher scoped listing, lookup and history queries were slower than direct reads of already-decrypted in-memory JSON at these sizes. Named-project deletion was slower at the smallest size because the SQLCipher transaction has fixed sync and cascade costs.

Passphrase unlock was slower with SQLCipher in this debug benchmark. The new wrapper runs Argon2id with 64 MiB and three iterations, versus about 19 MiB and two iterations in the old code. This comparison measures shipped policies, so the open-time difference does not isolate the storage engine. Absolute release-build timings, cold caches, larger or more diverse projects and different storage devices can change the results. The file-size gain also reflects integer-array expansion in the old ciphertext envelope. Migration temporarily increases disk use because an encrypted recovery file is intentionally retained.

Resident memory was not measured reliably per operation. RSS in this harness would combine both engines and Argon2 allocation with unrelated test-runner use. The old Vault kept decrypted data in memory and cloned it for each save. SQLCipher keeps a connection page cache and uses row-local queries, but full history/identity listings still allocate frontend-facing data.
