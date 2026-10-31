# Fokusgenmåling fra en gemt måling

En genmåling gemmes som en ny, selvstændig måling med egen rod, tid og dækning.
Den opdaterer aldrig det oprindelige snapshot, et eksisterende diagram eller
referencerne/totallen i en tidligere Hele Mac’en-session.

I en gemt måling viser topknappen **Genmål hele målingen** ved roden,
**Genmål denne mappe** i en undermappe og **Genmål denne fil** ved en gemt
filrod. Rodens brødkrumme giver fortsat adgang til genmåling af hele den
oprindelige rod. Hele Mac’en beholder sin eksisterende eksplicitte scanhandling.
En konkret kandidat under Indsigter har **Genmål dette område…**; handlingen
bruger kandidatens `drilldown_entry_id`, der er afgrænset til det valgte område.
Klassifikationens evidens kan ligge højere oppe og bruges aldrig som mål.

Dialogen viser den valgte gemte sti og kildens tidspunkt. Kun **Start genmåling**
starter arbejdet. Annuller og Escape starter intet; sene klargøringssvar ignoreres.
Den eksisterende jobstatus, Stop og publiceringsrutine bruges. Genmålknapperne
er deaktiverede under et job; den native joblås afviser også samtidige starter.

## Native- og Store-kontrakt

- `prepare_rescan {scanId, entry}` læser kun gemte metadata. Svaret indeholder
  `source_scan_id`, `source_entry_id`, `source_started_at`, `root_display`,
  `root_path_bytes_hex` og `kind`. Det starter intet job og validerer ikke
  kildens aktuelle eksistens. Kun gemte mapper og almindelige filer understøttes.
- `start_rescan {scanId, entry}` accepterer kun den gemte identitet. Stien udledes
  igen gennem Store Inspect-kontrakten og dens rå `path_bytes_hex`.
  UI'ets visningssti er aldrig input til scanneren. Den gentagne Inspect-klargøring
  kører via det eksisterende async/blocking-kald; scanneren genopløser også
  identiteten i sin eksisterende workertråd.
- `Store::rescan_to_store` genbruger den eksisterende streamingrutine og
  `ScanOptions::full()`. Native jobstatus bruger tekst med erstatningstegn plus
  rå hexmetadata; den serialiserer aldrig en non-UTF8 `PathBuf` direkte til JSON.
- Den eksisterende walker afviser en manglende eller direkte symbolsk rod.
  Fokusvarianten afviser også en ændret kanonisk destination, herunder et
  ancestor-alias ind i et providerlignende område. Kontrollen ligger efter de
  eksisterende provider-/dataless-gates og før åbning/oplistning af roden.
  Der tilføjes ingen særskilt forhåndsoplistning eller ny providerpolitik.

Manifestet får dette valgfrie felt **før** atomisk publicering:

```json
{
  "origin": {
    "source_scan_id": "<saved scan ID>",
    "source_entry_id": "<decimal entry ID>",
    "source_started_at": "<saved source measurement time>"
  }
}
```

Det indgår ikke i policyfingeraftrykket, traversal eller størrelser. Ældre
manifester uden feltet er fortsat læsbare. Den nye visning eksponerer `origin`
og den gemte `measurement_context`; historikken eksponerer rå rodidentitet og
kontekst til filtrering af sammenligningskandidater.

**Åbn oprindelig måling/post** fungerer fra den gemte provenance efter genstart.
Kilden slås op efter ID, også hvis den er uden for de seneste 100 historikposter.
Kildevisningen hentes, før den aktuelle visning ændres. En manglende kilde vises
eksplicit, og den nye måling kan stadig bruges. Sene kildesvar efter anden
navigation ignoreres.

Sammenligning kræver samme rå rod, policy, OS og stabil volumenidentitet samt
kronologisk rækkefølge. UI'et bruger gemte nanosekundtider, så to målinger i
samme viste sekund stadig kan sammenlignes. Store udfører fortsat sin endelige
kompatibilitetskontrol. En oprindelig måling af en bredere rod kan ikke
sammenlignes direkte med fokusmålingen; en gentagelse af fokusroden kan.
Provenance ændrer ikke denne kompatibilitet. Fald i målte størrelser er ikke
løfter om sikker sletning eller frigørbar APFS-plads.

## Verifikation og begrænsninger

`crates/analysis/tests/focused_rescan.rs` bruger egne syntetiske træer til
rå rodidentitet, immutable originalbytes, provenance ved publicering, samme-rod
sammenligning, manglende/symbolske/ændrede rødder samt Stop og metadata-budgetfejl
uden ny offentliggjort måling. Den native test kontrollerer klargøring uden job,
den aktive joblås og sikker non-UTF8-status ved en manglende rod.

Denne Macs native APFS afviser oprettelse af ugyldige UTF-8-filnavne med EILSEQ.
Non-UTF8-testen konstruerer derfor et gemt snapshot med sådanne rå stibytes,
verificerer tabsfri klargøring/status og afvisning af det manglende live-mål.
Det er ikke en påstand om en gennemført live-scan af et non-UTF8-træ på APFS.

`ui/focused-rescan.browser.mjs` kører browserforløbet med syntetiske native-svar
og almindelige ikke-tomme børn. Den dækker knaptekster, bekræftelse/annullering,
forældede svar, indsigtskandidatens afgrænsede ID, gamle visningers kildepar under
ventende navigation, jobkontroller, selvstændigt resultat, provenance efter reload,
manglende kilde med fortsat korrekt metric/genmåling og filvisning. Det er ikke
et native slutbrugeracceptforløb. Fildataindhold læses ikke af scanneren.

## Eksplicit native-fixture til accept

Den ignorerede test `prepare_native_focused_fixture` er et opt-in-værktøj.
Sæt `MACSCAN_FOCUSED_FIXTURE` til den absolutte `evidence/issue-13`-mappe og kør:

```sh
cargo test -p macscan-analysis --test focused_rescan prepare_native_focused_fixture -- --ignored --exact --nocapture
```

Den kræver friske `native-source` og `native-history` og overskriver intet.
Den opretter kun sit eget lille `source/project/Cargo.toml`-træ med fire genererede
filer i `target/debug`, en almindelig fil og en ejermarkør. Baseline gemmes med
den rigtige streaming-API og den lokale OS-/volumenidentitet; træet bevares.
`evidence/issue-13/native-fixture.json` indeholder stier, baseline-ID, projekt-/target-ID,
forventede tællinger, kandidatrapport og baseline-manifeststi. Den lokale
leveringsrapport tilføjer SHA-256 af det oprindelige manifest.

Native accept (udføres af MAIN, ikke fixturetesten):

1. Åbn den byggede app med `--store <native-history>` eller vælg den mappe som historik.
2. Åbn baseline, vælg Indsigter → Rust-buildoutput → Genmål dette område….
3. Kontrollér dialogens præcise `native-source/project/target` og source-tid,
   start eksplicit, og afvent det nye selvstændige resultat (6 poster).
4. Åbn oprindelig måling/post og kontrollér baseline og target-posten.
5. Gentag eventuelt genmålingen ved den nye rod og kontrollér samme-rod
   sammenligning. Den bredere baseline tilbydes ikke direkte som kandidat.
6. Kontrollér baseline-manifestets SHA-256 igen. Bevar kildefilerne indtil accept.

Efter accept kan kun den navngivne ejede kilde fjernes med denne begrænsede
kommando (historik og evidens bevares). Angiv den faktiske absolutte evidence-mappe:

```sh
python3 - /absolute/path/to/macscan/evidence/issue-13 <<'PY'
from pathlib import Path
import shutil, sys
base = Path(sys.argv[1])
assert base.is_absolute() and str(base).endswith('/evidence/issue-13')
source = base / 'native-source'
assert not source.is_symlink() and source.is_dir()
assert source.resolve() == base.resolve() / 'native-source'
assert (source / '.macscan-owned-fixture').read_bytes() == b'issue-13-native-source-v1\n'
shutil.rmtree(source)
PY
```
