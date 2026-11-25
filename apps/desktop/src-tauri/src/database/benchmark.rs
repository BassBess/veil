//! Synthetic comparison against the test-only legacy writer.
use super::{Mode, Vault, database_path, inspect};
use crate::{
    protection::{SessionVault, approved_detections},
    review::ReviewedEntity,
    vault_storage::UnlockedVault,
};
use std::{fs, hint::black_box, time::Instant};

const PASSWORD: &str = "synthetic benchmark passphrase";
const OPERATIONS: [&str; 9] = [
    "open", "projects", "lookup", "insert", "reuse", "process", "history", "clear", "delete",
];

fn timed<T>(sample: &mut Vec<f64>, run: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = black_box(run());
    sample.push(start.elapsed().as_secs_f64() * 1000.0);
    result
}

fn example(mapping_count: usize) -> (SessionVault, String, String) {
    let mut data = SessionVault::default();
    let project = data.create_project("Synthetic A".into()).unwrap();
    data.create_project("Synthetic B".into()).unwrap();
    data.create_project("Synthetic C".into()).unwrap();
    let mut lookup = String::new();
    for i in 0..mapping_count {
        let scope = if i % 4 == 1 { &project } else { "default" };
        let value = format!("Synthetic Mapping {i:04} with a regular repeated shape");
        let identity = data
            .update_project(Some(scope), |scope| {
                scope.save_identity("PERSON", &value, "")
            })
            .unwrap();
        if scope == "default" {
            lookup = identity.id;
        }
    }
    for _ in 0..15 {
        for scope in ["default", project.as_str()] {
            data.update_project(Some(scope), |scope| {
                scope.process(document(), spans(), "pseudonymize")
            })
            .unwrap();
        }
    }
    (data, project, lookup)
}

fn document() -> String {
    format!("Synthetic Record {}", "fictional filler ".repeat(300))
}

fn spans() -> Vec<ReviewedEntity> {
    vec![ReviewedEntity {
        id: "example".into(),
        entity_type: "PERSON".into(),
        text: "Synthetic Record".into(),
        start: 0,
        end: 16,
    }]
}

#[test]
#[ignore = "synthetic timing benchmark; run explicitly and serially"]
fn compare_legacy_and_sqlcipher() {
    for size in [25, 250, 1000] {
        let (data, project, lookup) = example(size);
        let mut legacy: [Vec<f64>; 9] = std::array::from_fn(|_| Vec::new());
        let mut cipher: [Vec<f64>; 9] = std::array::from_fn(|_| Vec::new());
        let (mut legacy_bytes, mut cipher_bytes) = (0, 0);
        for _ in 0..5 {
            let temp = tempfile::tempdir().unwrap();
            let old_path = temp.path().join("old/identities.vault");
            let new_path = temp.path().join("new/identities.vault");
            let mut old = UnlockedVault::open(old_path.clone(), PASSWORD, true).unwrap();
            old.update(|current| {
                *current = data.clone();
                Ok(())
            })
            .unwrap();
            drop(old);
            fs::create_dir(new_path.parent().unwrap()).unwrap();
            legacy_bytes = fs::copy(&old_path, &new_path).unwrap();
            let vault = Vault::open(
                new_path.clone(),
                inspect(&new_path).unwrap(),
                PASSWORD,
                false,
                Mode::Passphrase,
            )
            .unwrap();
            cipher_bytes =
                fs::metadata(database_path(&new_path, vault.bootstrap.generation).unwrap())
                    .unwrap()
                    .len()
                    + fs::metadata(&new_path).unwrap().len();
            drop(vault);

            let mut old = timed(&mut legacy[0], || {
                UnlockedVault::open(old_path, PASSWORD, false).unwrap()
            });
            let mut new = timed(&mut cipher[0], || {
                Vault::open(
                    new_path.clone(),
                    inspect(&new_path).unwrap(),
                    PASSWORD,
                    false,
                    Mode::Passphrase,
                )
                .unwrap()
            });
            timed(&mut legacy[1], || {
                let mut summaries = vec![(
                    "default".to_string(),
                    "General".to_string(),
                    old.data.records.len(),
                    old.data.pseudonyms.len(),
                )];
                summaries.extend(old.data.projects.iter().map(|p| {
                    (
                        p.id.clone(),
                        p.name.clone(),
                        p.data.records.len(),
                        p.data.pseudonyms.len(),
                    )
                }));
                summaries
            });
            timed(&mut cipher[1], || new.repo.projects().unwrap());
            timed(&mut legacy[2], || {
                old.data
                    .pseudonyms
                    .iter()
                    .find(|entry| entry.id == lookup)
                    .unwrap()
                    .clone()
            });
            timed(&mut cipher[2], || {
                new.repo.mapping("default", &lookup).unwrap()
            });
            let value = "A formerly unmapped synthetic value";
            for operation in [3, 4] {
                timed(&mut legacy[operation], || {
                    old.update(|data| data.save_identity("PERSON", value, ""))
                        .unwrap()
                });
                timed(&mut cipher[operation], || {
                    new.repo.save_mapping("default", "PERSON", value).unwrap()
                });
            }
            timed(&mut legacy[5], || {
                old.update(|data| data.process(document(), spans(), "pseudonymize"))
                    .unwrap()
            });
            timed(&mut cipher[5], || {
                let text = document();
                let approved = approved_detections(&text, spans(), "pseudonymize").unwrap();
                new.repo.process("default", text, approved, None).unwrap()
            });
            timed(&mut legacy[6], || {
                old.data
                    .records
                    .iter()
                    .rev()
                    .map(|record| record.public.clone())
                    .collect::<Vec<_>>()
            });
            timed(&mut cipher[6], || new.repo.records("default").unwrap());
            timed(&mut legacy[7], || {
                old.update(|data| {
                    data.records.clear();
                    Ok(())
                })
                .unwrap()
            });
            timed(&mut cipher[7], || {
                new.repo.clear_history("default").unwrap()
            });
            timed(&mut legacy[8], || {
                old.update(|data| data.delete_project(&project, "Synthetic A"))
                    .unwrap()
            });
            timed(&mut cipher[8], || {
                new.repo.delete_project(&project, "Synthetic A").unwrap()
            });
        }
        println!(
            "mappings={size} legacy_bytes={legacy_bytes} sqlcipher_with_bootstrap_bytes={cipher_bytes}"
        );
        for (index, operation) in OPERATIONS.iter().enumerate() {
            let old = &mut legacy[index];
            let new = &mut cipher[index];
            old.sort_by(f64::total_cmp);
            new.sort_by(f64::total_cmp);
            println!(
                "{operation:>8} legacy {:.3} [{:.3}–{:.3}] SQLCipher {:.3} [{:.3}–{:.3}] ms",
                old[2], old[0], old[4], new[2], new[0], new[4]
            );
        }
    }
}
