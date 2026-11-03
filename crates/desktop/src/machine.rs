//! Native adapter; orchestration and APFS accounting live in the shared analysis crate.
pub use macscan_analysis::machine::{mounts,plan,run,load};
#[cfg(test)]
use macscan_analysis::{Store,machine::publish};
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use std::{fs,path::{Path,PathBuf},sync::atomic::AtomicBool};

#[cfg(test)]
mod coverage_fixture {
    use super::*;
    // Opt-in only: copy already synthetic saved metadata, never plan or walk roots.
    #[test]
    #[ignore = "Writes synthetic saved whole-Mac coverage acceptance history"]
    fn write_native_machine_coverage_fixture() {
        let source=PathBuf::from(std::env::var_os("MACSCAN_COVERAGE_SOURCE").expect("synthetic source history"));
        let output=PathBuf::from(std::env::var_os("MACSCAN_COVERAGE_FIXTURE_OUT").expect("fixture output"));
        assert!(!output.exists(),"fixture output must be fresh");
        let source_store=Store::open(&source,false).unwrap();
        let mut ids=source_store.ids().unwrap();ids.sort();assert_eq!(ids,vec!["660-1","660-2","660-3"]);
        for id in ["660-1","660-2","660-3"] {
            assert_eq!(source_store.manifest(id).unwrap()["scope"]["root_display"],"/synthetic/macscan-coverage");
        }
        fn copy_metadata(source:&Path,target:&Path) {
            fs::create_dir(target).unwrap();
            for item in fs::read_dir(source).unwrap() {
                let item=item.unwrap();let kind=item.file_type().unwrap();assert!(!kind.is_symlink());
                let target=target.join(item.file_name());if kind.is_dir(){copy_metadata(&item.path(),&target);}else{fs::copy(item.path(),target).unwrap();}
            }
        }
        copy_metadata(&source,&output);
        let mut rows=Vec::new();
        for (i,(phase,status,scan)) in [("done",Some("complete"),Some("660-1")),("done",Some("partial"),Some("660-2")),("error",None,None),("unmounted",None,None),("cancelled",None,None),("not_scanned",None,None),("pending",None,None)].into_iter().enumerate() {
            rows.push(json!({"uuid":format!("synthetic-{i}"),"device":format!("synthetic{i}"),"name":format!("Synthetic {phase} {i}"),"root":if phase=="unmounted"{None}else{Some("/synthetic/macscan-coverage")},"phase":phase,"scan_status":status,"scan_id":scan,"os_used_bytes":"10000000000","entries":scan.map(|id|source_store.manifest(id).unwrap()["entry_count"].clone()),"roles":[],"branches":[],"error":if phase=="error"{Some("Synthetic execution failure; no source was accessed")}else{None}}));
        }
        let entry_count:u64=rows.iter().filter_map(|r|r["entries"].as_u64()).sum();assert_eq!(entry_count,6);
        let session=json!({"kind":"whole_mac_v1","session_id":"661-1","started_at":"2026-10-31T12:15:00Z","ended_at":"2026-10-31T12:15:01Z","status":"partial","entry_count":entry_count,"capacity_bytes":"1000000000000","used_bytes":"800000000000","free_bytes":"200000000000","volumes":rows,"coverage_note":"Syntetisk gemt oversigt; ukendte volumener er ikke nul hændelser."});
        publish(&output,&session).unwrap();assert_eq!(load(&output,"661-1").unwrap(),session);
        let store=Store::open(&output,false).unwrap();
        for id in ["660-1","660-2","660-3"] {
            for reason in [None,Some("access_denied")] {
                assert_eq!(crate::coverage_at(&output,id,reason,None).unwrap(),store.coverage(id,reason,None,10,&AtomicBool::new(false)).unwrap());
            }
        }
        fs::write(output.parent().unwrap().join("fixtures.json"),serde_json::to_vec_pretty(&json!({"source":"directly constructed synthetic saved metadata, copied from run A; no scans","measurement_ids":["660-1","660-2","660-3"],"machine_session":session})).unwrap()).unwrap();
    }
}
