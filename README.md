# Macscan

Lokal metadata-scanner i Rust til macOS/Apple Silicon. Gemmer komprimerede
Parquet-snapshots og bruger indlejret DuckDB til SQL og sammenligning.
CLI og en lokal Tauri-app med interaktiv sunburst. Ingen sletning, automatisk
oprydning, filindhold, autostart eller database-server.

## Mac-app

**Hele Mac’en** scanner monterede APFS-volumener på den fysiske opstartsdisk,
med separate resultater for Data, system, Preboot, VM osv. Umonterede volumener
vises med macOS’ rapporterede forbrug og markeres som ikke filscannede.
Eksterne drev og monterede diskimages/simulatorer lægges ikke oven i SSD-forbruget.
Samlet diskforbrug kommer fra APFS-containerregnskabet; filsummer er et separat
analysegrundlag, ikke et ekstra fysisk forbrug eller garanteret frigørbare bytes.
Almindelige mappe-/volumenscanninger findes fortsat i sidepanelet. Systemroden
udelader macOS-firmlinks fra `/usr/share/firmlinks` samt monteringsområder;
deres data gennemgås ved de kanoniske volumenrødder i stedet.

Under scanning vises rodgrene og deres status, antal og foreløbige allokering.
Højst 64 grupper vises; alle filer tælles fortsat med, også små og dybtliggende.
Foreløbige størrelser er **før hardlink-korrektion**, og afventende er ikke nul.
Færdige volumener gemmes som normale Parquet-målinger. En lille privat
`*.machine.json` binder dem sammen i historikken, også efter Stop; en afbrudt
volumens ufærdige arbejdsfiler publiceres ikke. Tidligere resultater bevares.
Klik på en volumen for filanalyse og brug “Hele Mac’en”-brødkrummen for at gå op.

Streamingmotoren bruger macOS `getattrlistbulk` i 64 KiB batches til almindelige
filers metadata, med stat-opslag for mapper, symlinks og manglende attributter.
Filsystemer uden bulk-understøttelse falder tilbage til descriptor-relative opslag.
En lokal varm-cache-test af 10.000 tomme filer målte cirka 1,6–1,8 gange hurtigere
metadata-gennemgang; det er ikke et løfte om samme gevinst for en fuld scanning.
Der læses ikke filindhold. Scanning er fortsat enkelttrådet; DuckDB-finalisering
bruger to tråde. Fuld diskadgang skal brugeren selv give i macOS, hvis nødvendigt.
Ingen rettigheder ændres automatisk. Beskyttede/providerområder vises som delvise.
Knappen “Fuld diskadgang…” åbner kun den relevante side i Systemindstillinger.
Tilføj `dist/Macscan.app` dér manuelt, og genstart appen. Lokale ad-hoc-genbygninger
kan ugyldiggøre tidligere tilladelser; fjern og tilføj da appen igen. Appen har
formålsbeskrivelser for musikbibliotek og standardmapper, men giver sig ikke adgang.
Store scanninger hæver kun egen proces’ bløde grænse for åbne filer til højst8192
(aldrig over den eksisterende hårde grænse); systemets grænser ændres ikke.

Åbn `dist/Macscan.app`. Appen bruger samme Rust-scanner, Parquet og DuckDB som
CLI'en. Klik på en farvet mappe for at gå ind, på midten eller brødkrummerne
for at gå op. Mappelisten er også fuldt tastaturbetjent. Filklik viser metadata;
appen åbner aldrig automatisk en fil. Kopiér sti og Vis i Finder er eksplicitte
handlinger. Der er ingen sletteknapper.

Højreklik på en sunburst-blok eller en post i mappelisten for **Vis i Finder**,
**Kopiér filsti** og **Kopiér information**. Information kopieres som JSON med
sti, type, størrelser, dækning og måletidspunkt fra den gemte måling, ikke live
fildata. I listen åbner Shift+F10 også menuen. “Øvrige poster” er en samleblok;
vælg en konkret post i listen for at få filhandlinger.

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
Det samlede historikbudget er 4 GiB uden automatisk sletning af gemte målinger.
En ny måling kan afvises, når budgettet er opbrugt; CLI kan ændre budgettet.

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

Root er altid eksplicit. Uden `--save` bruges en privat midlertidig måling,
som fjernes efter det kompakte svar; der beholdes ingen historik.
Standardhistorik er `~/Library/Application Support/Macscan`;
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
Gamle prototype-målinger kan stadig åbnes; den nye fulde scanpolitik er forskellig,
så en ny fuld måling sammenlignes med en anden fuld måling, ikke en gammel prototype.
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
`entries/part-*.parquet` (Zstandard, skrivegrupper på 8.192 poster og dele på omtrent 32 MB).
Der er ingen permanent DuckDB-database eller store JSONL-filer. Scanneren
afleverer små batches direkte til Parquet. En midlertidig diskbaseret DuckDB
beregner hardlinks og mappesummer, så hele filtræet ikke opbevares i RAM.
Arbejdsfiler fjernes efter succes, Stop og normale fejl. Egne markerede,
upublicerede arbejdsmapper fra en tidligere afbrudt proces ryddes ved næste scan;
gemte målinger og ukendte mapper slettes aldrig automatisk.
Den gamle in-memory/JSONL-prototype findes stadig til reference-tests, men
appen og CLI-scanning bruger den nye streamingmotor.

Historik har som standard **4 GiB samlet budget**, ændres eksplicit med
`--metadata-budget BYTES`. Ingen automatisk sletning af gamle målinger.
Gemning publiceres atomisk uden overskrivning; fejl fjerner kun eget staging.
Standardscan gennemgår hele den valgte lokale volumen/root uden en fast post-,
tids- eller 64-niveausgrænse. CLI kan stadig sætte eksplicit `--max-entries` og
`--max-duration`. OS-adgangsfejl, provider-/volumenudeladelser og eventuelle
ressourcefejl er synlige, ikke tavst komplette scanninger. Stop virker også
under slutberegningen. Entry-ID-formatet understøtter op til ca. 4,3 mia. poster.
Writerens Rust-batchbudget er højst 4 MiB plus åbne forfædre; DuckDB bruger
512 MiB bufferbudget ved streaming/finalisering og to-scan-sammenligning,
256 MiB ved almindelig analyse og to tråde. Det er **ikke** en grænse for hele processens RSS.
Kun slutberegningens private arbejdsområde tillader disk-spill. Arbejdsområdet
begrænses til højst 8 GiB (lavere ved lille metadata-budget), kontrolleres løbende,
og afbrydes, hvis der er under 512 MiB ledigt. Kontrollernes interval er 200 ms;
det er ikke en atomisk filesystem-kvote. Almindelige queries spilder ikke til disk
og kan derfor afvises ved deres hukommelsesgrænse.

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
Rust- og JS-tests samt eksplicitte million-entry tests verificerer det
aktuelle workflow. Live filsystemer kan ændre sig under scan; det er ikke et
atomisk APFS-snapshot. Brugt på egen Mac: ~415.500 poster på 14 sekunder,
26,5 MB metadata pr. snapshot. Milliontesten gav 12,3 MB for to snapshots;
kompressionsforholdet afhænger derfor stærkt af de faktiske navne og data.
