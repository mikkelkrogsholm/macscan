# Målehistorik: seneste tre scanninger

Macscan gemmer de seneste tre kørsler globalt i den valgte målemappe. En
mappe-, fil- eller fokuseret genmåling tæller som én kørsel. En hel Mac-scanning
og alle dens gemte volumenmålinger tæller tilsammen som én kørsel. Historikken
viser den samlede Mac-kørsel; volumenmålingerne åbnes fra denne oversigt.

Efter en ny gyldig måling er offentliggjort, fjernes ældre Macscan-metadata.
En delvis dækningsmåling er stadig en gyldig observation. En hel Mac-kørsel
rydder først op efter den samlede sessionsfil er gemt, aldrig mellem volumener.
Stop eller kørsels-/lagerfejl rydder ikke tidligere historik. En senere vellykket
kørsel kan rydde ældre, gemte delvise sessioner. Den netop gemte kørsel
beskyttes ved automatisk oprydning, også hvis uret er rettet bagud eller
importeret historik har fremtidige ID’er. Læsning, appstart og valg af
målemappe udløser ingen oprydning.

Kun genkendte, direkte Macscan-snapshotmapper og sessionsfiler slettes. Alle
referencer fra de tre bevarede sessioner beskyttes, også delte volumenmålinger.
Ukendte mapper, symbolske links og aktive arbejdsmapper slettes ikke af
oprydningen. Ugyldige eller ufuldstændige sessionsreferencer afbryder oprydning. En gyldig,
afsluttet session uden volumenmålinger efter Stop eller fejl tæller som en
kørsel ved senere oprydning.
Oprydning følger aldrig gemte stier til de scannede filer. Kildemålinger for en
fokuseret genmåling kan udgå; appen viser da, at kilden ikke er tilgængelig.

Hvis oprydning fejler efter offentliggørelse, er den nye måling stadig gemt og
kan åbnes. Appen viser en advarsel; CLI-resultatet får `retention_warning`.
Historikken kan derfor midlertidigt indeholde flere end tre kørsler.

## Kontrolleret oprydning fra CLI

```sh
macscan history-prune --dry-run --store /absolut/sti/til/maalinger
macscan history-prune --apply --store /absolut/sti/til/maalinger
```

Uden `--apply` slettes intet. Begge tilstande bruger samme planlægger, validering
og gruppering som automatisk oprydning. Manuel oprydning beholder de tre
numerisk nyeste ID’er; automatisk oprydning beskytter også den netop gemte
kørsel ved en eventuel urkorrektion. Resultatet indeholder eksakte antal kørsler,
snapshotmapper og sessioner, højst tre bevarede og otte fjernede ID'er i
oversigten. `removed_*` er planlagte antal ved dry-run og gennemførte antal ved
vellykket apply. JSON-output er højst 8 KiB. En anden igangværende scanning eller
oprydning i samme målemappe giver en fejl; prøv igen når den er færdig.

## Lagringsgrænser

Standardgrænsen er 4 GiB til den **nye måling**, ud over eksisterende metadata.
En hel Mac-kørsel deler én 4 GiB-tildeling mellem alle dens volumenmålinger.
Summen beregnes med overflow-kontrol under den fælles kørselslås. Arbejdsområdet
har fortsat højst 8 GiB og kræver 512 MiB fri plads. Oprydning øger ikke disse
grænser og kræver ikke, at ældre historik slettes før en ny måling lykkes.

CLI `scan --metadata-budget BYTES` og eksisterende Store-metoder med et
udtrykkeligt budget bevarer deres samlede grænse: eksisterende plus nye
metadata skal være inden for budgettet. `scan --save` rydder automatisk op efter
en vellykket offentliggørelse, også med et udtrykkeligt budget. En scanning uden
`--save` bruger en midlertidig mappe. Scannede brugerfiler slettes aldrig.
