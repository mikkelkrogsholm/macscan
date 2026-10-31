import test from 'node:test';
import assert from 'node:assert/strict';
import {categoryPresentation} from './categories.mjs';
test('contextual classification has Danish labels and optional technical evidence',()=>{
 const c=categoryPresentation({category:'build_output',category_rule_id:'rust_target',category_rule_version:'context-1',category_coverage:'partial',category_evidence_source:'saved_metadata:entry=4,marker=2'});
 assert.equal(c.label,'Buildoutput');assert.match(c.reason,/Sandsynligt Rust/);assert.equal(c.coverage,'Delvis dækning');assert.match(c.detail,/context-1.*marker=2/);
});
test('unknown and missing classification remains explicit',()=>{
 const c=categoryPresentation();assert.equal(c.label,'Ukendt kategori');assert.equal(c.coverage,'Kontekst mangler');assert.doesNotMatch(Object.values(c).join(' '),/undefined|null/);
});
