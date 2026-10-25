# Macscan

Lokal metadata-scanner i Rust til macOS/Apple Silicon. Gemmer komprimerede
Parquet-snapshots og bruger indlejret DuckDB til SQL og sammenligning.
CLI og en lokal Tauri-app med interaktiv sunburst. Ingen sletning, automatisk
oprydning, filindhold, autostart eller database-server.

## Mac-app

Åbn `dist/Macscan.app`. Appen bruger samme Rust-scanner, Parquet og DuckDB som
CLI'en. Klik på en farvet mappe for at gå ind, på midten eller brødkrummerne
for at gå op. Mappelisten er også fuldt tastaturbetjent. Filklik viser metadata;
appen åbner aldrig automatisk en fil. Kopiér sti og Vis i Finder er eksplicitte
handlinger. Der er ingen sletteknapper.

- Native mappevalg, scanning med observerede tællere og Stop.
- Gemte målinger med dato og tydelig “ikke live”-markering.
- Allokeret/logisk metric, navnesøgning, pagination og en synkroniseret inspektør.
- Sunburst med højst 320 noder og tre niveauer. Øvrige poster står i listen;
  en million filer bliver ikke til en million DOM-elementer.
- Ændringer sammenligner tidligere målinger af samme root. Grafen viser fortsat
  pladsfordelingen; listen viser fortegn og før-/efterændringer.
- Ufuldstændige scans viser kun observeret minimum og en synlig advarsel.

Brug “Målemappe” til at vælge den eksisterende `measurements/` fra dette projekt.
Valget huskes lokalt i appens WebView. Standardplaceringen er den samme som
CLI'ens. Appen kan også startes med `--store /absolute/history`.
UI'ens samlede historikbudget er 512 MiB uden automatisk sletning; CLI-standard
er fortsat 256 MiB. En ny måling kan afvises, når budgettet er opbrugt.

Byg med `scripts/build-desktop.sh`. Frontend er lokal HTML/CSS/JavaScript/SVG
uden npm-afhængigheder eller separat frontendbyg. Tauri er isoleret i sin egen
crate og er ikke en runtimeafhængighed for CLI'en. Native dialog bruger rfd.
Appen er lokalt ad-hoc-signeret, ikke Developer-ID-signeret/notariseret og ikke
installeret i Applications. Den bruger systemets WebView, ikke medpakket Chromium.
Sunburst-reference: [DaisyDisk](https://daisydiskapp.com/); egne assets/layout.
IPC følger [Tauri 2's kommando-API](https://v2.tauri.app/develop/calling-rust/).

## Brug

Kør fra denne mappe. `dist/macscan` og `dist/libduckdb.dylib` skal ligge sammen.
Binæren er lokalt ad-hoc-signeret, ikke globalt installeret/notariseret.

```sh
dist/macscan schema
dist/macscan capabilities
dist/macscan volumes
dist/macscan scan /absolute/folder --save --store /absolute/existing-parent/history
dist/macscan scans --store /absolute/existing-parent/history
dist/macscan children --scan SCAN_ID --entry 0 --store /absolute/existing-parent/history
dist/macscan largest --scan SCAN_ID --store /absolute/existing-parent/history
dist/macscan inspect --scan SCAN_ID --entry ENTRY_ID --store /absolute/existing-parent/history
dist/macscan summary --scan SCAN_ID --store /absolute/existing-parent/history
dist/macscan query --scan SCAN_ID --store /absolute/existing-parent/history \
  --sql "SELECT path_display,logical_bytes FROM entries WHERE kind='file' ORDER BY logical_bytes DESC,entry_id"
dist/macscan compare BEFORE_ID AFTER_ID --metric logical --store /absolute/existing-parent/history
```

Root er altid eksplicit. Uden `--save` får man kun et kompakt preview, ikke
historik. Standardhistorik er `~/Library/Application Support/Macscan`;
`--store` vælger en anden absolut placering, hvis parent eksisterer.
Placér historik uden for det scannede træ; ellers udelades den, og scan er partial.

Agentoutput: højst **8 KiB**, som standard **10 rækker**, `--limit 1..100`.
`--format json|ndjson`; progress er særskilt stderr med `--progress text|ndjson`.
Brug hele `next_cursor` med samme navigation/compare-query og `--cursor`.
SQL bruger `next_offset` og `--offset`; behold samme SQL og en stabil `ORDER BY`.
Bytes og SQL-tal returneres som præcise decimalstrenge; ukendte værdier er null.
Scan med begrænset dækning returnerer et brugbart resultat og exitkode 5.

## SQL, ikke en million JSON-rækker

Query læser direkte fra snapshottenes Parquet. Tabellen `entries` indeholder
bl.a. `entry_id`, `parent_id`, `path_display`, `kind`, `logical_bytes`,
`allocated_bytes`, `accounted_allocated_bytes`, `mtime_unix_seconds`,
`coverage_complete`, `category` og rå navne/stier som BLOB. Se `schema` for felter.

```sql
-- Ingen dobbeltoptælling af mapper + deres børn:
SELECT category, count(*) AS files,
       sum(accounted_allocated_bytes) AS bytes
FROM entries WHERE kind='file'
GROUP BY category ORDER BY bytes DESC,category;

-- Direkte børn af root:
SELECT entry_id,path_display,accounted_allocated_bytes
FROM entries WHERE parent_id=0
ORDER BY accounted_allocated_bytes DESC NULLS LAST,entry_id;
```

`query --scan AFTER --before BEFORE --sql 'SELECT ... FROM changes ...'`
tilføjer `before_entries` og `changes`. Sidstnævnte har før-/efterværdier og
`logical_delta`/`allocated_delta`. Kendte uændrede poster udelades fra `changes`;
de fulde uændrede tabeller findes fortsat som `entries` og `before_entries`.
Den indbyggede `compare` håndterer pagination
og ukendt dækning. Kun samme root, scanpolitik, OS og volumen kan sammenlignes.
Indlejrede mappeændringer overlapper: summer ikke alle compare-rækker.

## Hvad størrelserne betyder

`logical` er filernes længde. `allocated` i navigationen er hardlink-tilskrevet
OS-allokering inden for root, **ikke en garanti for frigørbar APFS-plads**.
Sparse filer måles særskilt; hardlinks tilskrives én ejer. Private/unikke
APFS-bytes er endnu ukendte. Ufuldstændige totaler er null med observerede
minimumsværdier, ikke nul. Manglende adgang må ikke ligne slettede filer i diff.

Kategorier som cache, build_output og dependencies er navnebaserede hints,
ikke godkendelse til sletning. Metadata kan vise hvor væksten ligger, men
beviser ikke hvilken proces eller person der skabte den.
Kendte cloud-roots udelades; fuld provider-/full-disk-certificering mangler.

## Historik og ressourcegrænser

Hver måling har en lille `manifest.json`, `coverage.parquet` og
`entries/part-*.parquet` (Zstandard, 8.192 poster pr. del).
Ingen permanent DuckDB-database eller store JSONL-filer. DuckDB åbnes kun under
gemning/query. Den gamle JSONL-implementering i engine er ikke CLI'ens format.

Historik har som standard **256 MiB samlet budget**, ændres eksplicit med
`--metadata-budget BYTES`. Ingen automatisk sletning af gamle målinger.
Gemning publiceres atomisk uden overskrivning; fejl fjerner kun eget staging.
Standardscan: højst 1.000.000 poster/300 sekunder/64 katalogniveauer.
DuckDB bruger 128 MiB bufferbudget ved skrivning og 256 MiB ved analyse,
512 MiB ved to-scan-sammenligning, to tråde og ingen temp-spill. Det er **ikke**
en grænse for hele processens RSS.
En query kan derfor afvises ved hukommelsesgrænsen; den fylder ikke disken med spill.

## Byg

Kræver macOS 14+ på Apple Silicon, Xcode Command Line Tools, Rust 1.99.1
via rustup og Node.js på `/opt/homebrew/bin/node` til geometriske tests.
Buildscripts bruger rustup-toolchainen i `~/.rustup/toolchains/`.
På en frisk kloning hentes Rust-afhængigheder én gang, før de låste
offline-builds køres:

```sh
rustup toolchain install 1.99.1 --profile minimal
cargo fetch --locked
cargo install tauri-cli --version 2.12.4 --locked  # kun til Mac-appen
scripts/fetch-duckdb.sh   # én eksplicit download af låst, checksum-verificeret bibliotek
scripts/build-local.sh   # låste offline-tests, release og lokal signering
scripts/build-desktop.sh # Tauri-app i dist/Macscan.app
```

Bruger eksisterende Rust 1.99.1; deployment target macOS 14.0. DuckDB v2.0.0
arm64 fra officiel release, indlejret via C API. Ingen Python/Node/DB-server i
leverancen. CLI er ca. 0,9 MB; DuckDB-biblioteket ca. 63 MiB og skal medregnes.
Motoren alene er lille; denne kombination er ikke en sub-MB-distribution.

Lokale diskmålinger, driftsnoter, test-/målebeviser og byggede binærer er
udeladt fra Git. Egne målinger gemmes lokalt; de uploades ikke af appen.
Det er en brugbar lokal prototype, ikke accept af alle oprindelige V1-/UI-krav.
49 Rust-tests og fem geometriske JS-tests samt en million-entry test verificerer det
aktuelle workflow. Live filsystemer kan ændre sig under scan; det er ikke et
atomisk APFS-snapshot. Brugt på egen Mac: ~415.500 poster på 14 sekunder,
26,5 MB metadata pr. snapshot. Milliontesten gav 12,3 MB for to snapshots;
kompressionsforholdet afhænger derfor stærkt af de faktiske navne og data.
