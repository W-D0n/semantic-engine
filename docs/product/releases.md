# Distribution et releases

Semantic Engine distingue une **prévisualisation technique** d’une **release
publique**. Les deux passent les mêmes tests et contrôles d’intégrité, mais seule
une release possède une licence de code explicite et peut être redistribuée.

## Artefacts produits

Le workflow `Distribution` construit les artefacts suivants sans dépendre de
MyVault, Answer Atlas ou Media Catalog :

| Artefact | Runner natif | Usage |
|---|---|---|
| CLI Windows x64 | `windows-latest` | sidecar JSONL, serveur headless, évaluation |
| CLI Linux x64 | `ubuntu-latest` | intégration locale ou serveur headless |
| CLI macOS arm64 | `macos-latest` | intégration locale Apple Silicon |
| application Windows x64 légère | `windows-latest` | machine avec WebView2 système |
| application Windows x64 hors ligne | `windows-latest` | dossier portable avec WebView2 fixe |

Chaque paquet CLI inclut les contrats publics, les clients de conformité et un
contexte de démarrage. Il contient aussi `release-manifest.json` et
`SHA256SUMS.txt`. La portable Windows possède ses checksums internes ; la release
ajoute des checksums externes et un SBOM CycloneDX.

## Prévisualisation sans licence

Depuis GitHub Actions, lancer manuellement **Distribution**. L’option
`include_windows_portable` ajoute la portable Windows, beaucoup plus volumineuse.
Les paquets produits sont conservés 14 jours et portent
`PREVIEW-NOT-LICENSED.txt` : ils servent à valider le produit, pas à le
redistribuer.

En local :

```powershell
cargo build --locked --release -p semantic-engine-cli --bin semantic-engine-cli
node scripts/package-cli.mjs `
  --binary target/release/semantic-engine-cli.exe `
  --platform windows --arch x64 `
  --output artifacts/semantic-engine-cli-0.1.0-windows-x64 `
  --commit (git rev-parse HEAD)
node scripts/verify-checksums.mjs artifacts/semantic-engine-cli-0.1.0-windows-x64
```

## Garde-fous d’une release publique

Un tag `v0.1.0` ne peut produire une release que si :

1. les versions Cargo, npm et Tauri sont toutes `0.1.0` ;
2. une licence `LICENSE` existe à la racine ;
3. `CHANGELOG.md` contient une section versionnée `## [0.1.0]` ;
4. le quality gate du corpus passe sur chaque CLI native ;
5. les sommes de chaque paquet sont recalculées par un second script ;
6. les builds Windows portable et léger aboutissent avec le runtime verrouillé.

Le workflow crée alors une **release GitHub en brouillon**. Il refuse de modifier
une release déjà publiée. La publication finale reste une action humaine après
revue de la licence, des notes, des artefacts et de la signature native.

Pour demander explicitement un paquet public en local, ajouter `--release` à
`package-cli.mjs` ou `-Release` à `build-portable.ps1`. Ces options échouent avant
la compilation ou la copie si `LICENSE` est absente.

## Vérifier un téléchargement

Vérifier d’abord les checksums du paquet extrait :

```powershell
node scripts/verify-checksums.mjs C:\chemin\semantic-engine-cli-0.1.0-windows-x64
```

Vérifier ensuite que l’archive publiée a bien été produite par le workflow de ce
dépôt :

```powershell
gh attestation verify semantic-engine-cli-0.1.0-windows-x64.tar.gz `
  --repo W-D0n/semantic-engine
```

Les attestations GitHub fournissent une provenance signée et relient les
exécutables Windows à leur SBOM. Elles ne remplacent pas une signature de code
native : Windows ne les utilise pas pour l’éditeur affiché par SmartScreen ou
les propriétés Authenticode. La case « release signée » de la roadmap restera
donc ouverte jusqu’à la signature native des exécutables distribués.

## Références

- [GitHub — Utiliser les attestations d’artefacts](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)
- [GitHub — `actions/attest`](https://github.com/actions/attest)
- [GitHub — artefacts de workflow](https://docs.github.com/en/actions/concepts/workflows-and-actions/workflow-artifacts)
- [GitHub — runners hébergés](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
