# Index public, signatures et révocation des paquets de contexte

- Statut : recommandation d'architecture, à transformer en profil et contrats
- Recherche vérifiée le : 2026-08-02
- Périmètre : Semantic Engine, Answer Atlas et producteurs tiers

## Décision recommandée

Publier chaque source de paquets comme un **canal statique TUF** dont les cibles
sont des archives **Data Package v2** immuables.

La v1 doit implémenter le cycle TUF complet (`root`, `targets`, `snapshot`,
`timestamp`), pas un « TUF-lite » réduit à un index signé. Le coût est quatre
rôles de métadonnées et un renouvellement périodique de Timestamp ; le bénéfice
est de ne pas devoir réinventer — ni reporter — rollback, freeze, cohérence des
instantanés, rotation et révocation de clés. Le CLI masque ce coût aux
producteurs.

- Data Package reste le format métier lisible et diffusable :
  `datapackage.json`, ressources, sources, licence et profil Semantic Engine.
- [The Update Framework (TUF)](https://theupdateframework.github.io/specification/latest/)
  fournit l'index signé et le cycle de confiance : racine, délégations, versions,
  expiration, seuils de signatures, rotation et révocation de clés.
- SHA-256 dans les métadonnées TUF identifie les octets exacts de l'archive. Le
  profil Semantic Engine continue d'imposer `sha256:` pour chaque ressource du
  Data Package ; le standard Data Resource autorise explicitement le préfixe de
  l'algorithme dans `hash`.
- Une attestation DSSE/Sigstore peut compléter les canaux publics importants
  pour l'identité et la transparence. Elle ne remplace ni TUF ni la politique de
  confiance locale.

Cette composition réutilise les standards pour ce qu'ils savent déjà faire. Il
ne faut pas créer une enveloppe cryptographique Semantic Engine, ni signer
directement un `datapackage.json` avec un protocole ad hoc.

## Deux niveaux qui ne doivent pas être confondus

```text
canal TUF (authenticité, fraîcheur, révocation)
└── cible immuable .zip (taille + SHA-256)
    └── Data Package v2 (identité, licence, provenance, ressources)
        └── données Semantic Engine (cibles et alias)
```

Le [Data Package Standard](https://datapackage.org/standard/data-package/)
impose `datapackage.json` à la racine d'un paquet publié et recommande le profil
2.0. Il ne définit pas l'authenticité de l'éditeur ni la sécurité des mises à
jour. Inversement, TUF traite une cible comme un fichier opaque : il peut donc
sécuriser un ZIP Data Package sans connaître son contenu.

## Profil public proposé : Context Channel v1

### Un canal est une frontière de confiance autonome

Un canal est un dépôt TUF standard accessible par HTTPS, GitHub Pages, stockage
objet, serveur statique ou dossier importé hors ligne :

```text
channel/
├── metadata/
│   ├── root.json
│   ├── 1.root.json
│   ├── targets.json
│   ├── snapshot.json
│   ├── timestamp.json
│   └── <rôles-délégués>.json
└── targets/
    ├── packages/<publisher>/<package>/<semver>.zip
    ├── channel-profile.json
    └── revocations-v1.json
```

Les noms physiques versionnés ou préfixés par un hash doivent être générés par
l'implémentation TUF lorsque `consistent_snapshot` est actif, pas reconstruits
manuellement. Les chemins logiques de cibles utilisent `/`, restent relatifs et
ne commencent jamais par `/`, conformément au format `TARGETPATH` de TUF.

Semantic Engine accepte plusieurs canaux indépendants. Answer Atlas est un canal
officiel préconfiguré, pas une autorité obligatoire pour tout l'écosystème. Un
producteur tiers peut :

1. publier son propre petit canal statique et faire approuver sa racine par
   l'utilisateur ; ou
2. recevoir une délégation limitée, par exemple
   `packages/<publisher>/*`, dans un canal communautaire.

Les [délégations TUF](https://theupdateframework.io/docs/metadata/) associent des
clés et un seuil à des motifs de chemins ; un producteur délégué ne peut donc pas
publier dans l'espace d'un autre. Une liste publique de canaux peut faciliter la
découverte, mais elle ne doit jamais accorder silencieusement la confiance : la
racine du canal reste le point d'autorisation.

### Métadonnées d'index

Chaque archive est une entrée de `targets.json` ou d'un rôle délégué. TUF signe
le chemin, la longueur et les hashes. Le profil de canal, lui-même cible TUF,
énumère les chemins à découvrir et les métadonnées nécessaires à l'aperçu. Ce
choix fonctionne avec TUF-on-CI, qui reconstruit automatiquement les cibles sans
injecter de champ `custom` lors d'une première publication :

```json
{
  "packages": [{
    "path": "answer-atlas-core-titles-1.2.0.zip",
    "metadata": {
      "profile": "context-target-v1",
      "packageId": "urn:answer-atlas:core-titles",
      "packageName": "core-titles",
      "packageVersion": "1.2.0",
      "formatVersion": "0.1.0",
      "kind": "recognition-context",
      "locales": ["en", "fr"],
      "kinds": ["game", "movie"],
      "targetCount": 84,
      "spdxLicenseExpression": "CC0-1.0"
    }
  }]
}
```

Règles du profil :

- `packageId`, `packageName`, `packageVersion`, `formatVersion`, `kind`,
  `locales` et `spdxLicenseExpression` reprennent les champs du Data Package ;
- après téléchargement, toute divergence entre l'index signé et
  `datapackage.json` provoque un refus, même si les hashes sont valides ;
- la clé ou le rôle TUF qui a autorité sur le chemin détermine l'éditeur de
  sécurité. Un champ texte `publisher` auto-déclaré ne suffit jamais ;
- une version publiée est immuable. Corriger les octets exige une nouvelle
  version de paquet et une nouvelle cible ;
- les versions entières des métadonnées TUF sont indépendantes du SemVer du
  paquet ;
- longueurs, chaînes, nombre de cibles, délégations et téléchargements restent
  bornés avant allocation ou extraction.

Le [format des cibles TUF](https://github.com/theupdateframework/specification/blob/master/tuf-spec.md#file-formats-targets)
prévoit précisément `length`, `hashes` et un champ `custom` optionnel. Le profil
applicatif séparé évite de dépendre de ce champ optionnel. Les métadonnées Snapshot
lient les versions et hashes de toutes les métadonnées Targets, empêchant un
intermédiaire de fabriquer une combinaison qui n'a jamais existé ; Timestamp
référence le Snapshot récent et expire rapidement.

### Descripteur du canal

`channel-profile.json` est une cible TUF ordinaire. Il contient le nom affiché,
l'identifiant stable, la page de référence et la liste bornée des paquets. Il
n'est fiable qu'après validation TUF.

Lors du premier ajout d'un canal tiers, l'interface affiche avant toute
activation :

- l'URL ou le chemin local ;
- le SHA-256 des octets de la racine initiale ;
- la version de cette racine et ses seuils de clés ;
- le fait qu'il s'agit d'une nouvelle autorité pouvant publier des données.

Le nom convivial non encore authentifié peut aider l'utilisateur, mais ne doit
pas masquer cette empreinte. Il n'y a pas de TOFU silencieux.

## Révocation

La révocation de clés et celle d'un paquet sont deux opérations différentes.

### Clés et producteurs

- Une délégation producteur est révoquée en publiant une nouvelle métadonnée du
  rôle délégant qui ne contient plus la délégation.
- Une clé Targets, Snapshot ou Timestamp est remplacée dans une nouvelle
  `root.json`.
- Une rotation de racine est vérifiée version par version. Chaque nouvelle
  racine doit satisfaire à la fois le seuil de l'ancienne racine et son propre
  nouveau seuil. Les anciennes racines versionnées restent disponibles.

Ce mécanisme est celui défini par la section
[Key management and migration de TUF](https://github.com/theupdateframework/specification/blob/master/tuf-spec.md#key-management-and-migration).

### Paquets et contenus

La suppression d'une cible de l'index empêche une nouvelle découverte, mais ne
dit pas clairement à un client qu'une version déjà installée est dangereuse.
Chaque canal publie donc `revocations-v1.json` comme cible TUF :

```json
{
  "$schema": "urn:semantic-engine:context-revocations:1",
  "formatVersion": 1,
  "sequence": 7,
  "updatedAt": "2026-08-02T00:00:00Z",
  "entries": [
    {
      "archiveSha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "packageId": "urn:answer-atlas:core-titles",
      "packageVersion": "1.2.0",
      "effectiveAt": "2026-08-02T00:00:00Z",
      "reason": "invalid-data",
      "replacement": "1.2.1"
    }
  ]
}
```

Contraintes recommandées :

- l'identité d'une révocation est le SHA-256 exact de l'archive, complété par
  l'identité et la version pour l'affichage ;
- `sequence` augmente à chaque publication et le client conserve son plus haut
  niveau vu ;
- une empreinte révoquée reste dans un tombstone local, même si une liste future
  l'omet. Une correction se republie avec de nouveaux octets, un nouveau hash et
  une nouvelle version ;
- le client vérifie cette liste avant installation et avant activation ; après
  rafraîchissement, il désactive ou met en quarantaine toute version active
  nouvellement révoquée et explique la raison ;
- la liste est elle-même liée par `targets` → `snapshot` → `timestamp`. Elle ne
  doit pas être téléchargée depuis une URL annexe non authentifiée.

Un client totalement hors ligne ne peut pas connaître une révocation publiée
après son dernier état importé. L'UI doit donc dire « vérifié selon l'état du
<date/version> » et jamais « sûr » ou « à jour » sans rafraîchissement.

## Politique de clés proposée

TUF supporte Ed25519 et les seuils de signatures. Pour garder les outils simples,
le CLI éditeur doit générer et signer les quatre rôles sans exposer cette
complexité dans le flux normal.

| Canal | Root | Targets | Snapshot | Timestamp |
|---|---|---|---|---|
| Answer Atlas officiel | 2 parmi 3, hors ligne | clé distincte hors ligne | clé distincte hors ligne | clé distincte automatisable |
| Petit producteur tiers | 1 parmi 1, hors ligne | clé de publication distincte | peut partager la clé de publication au départ | clé distincte automatisable |

Le partage Targets/Snapshot est un compromis de démarrage pour un petit éditeur,
pas la configuration officielle. La clé Root ne signe jamais les publications
ordinaires. La clé Timestamp étant en ligne et plus exposée, TUF la sépare de
Snapshot, dont la clé peut rester hors ligne.

Durées initiales suggérées, à calibrer opérationnellement : Timestamp 7 jours,
Snapshot 30 jours, Targets 90 jours, Root 365 jours. Ce sont des choix produit,
pas des valeurs imposées par TUF. Une tâche de publication renouvelle Timestamp
avant expiration même lorsque le catalogue ne change pas.

## Flux du client portable et hors ligne

1. Charger la dernière racine approuvée depuis le stockage durable local.
2. Vérifier séquentiellement toutes les nouvelles racines contre les anciens et
   nouveaux seuils, puis les persister de façon atomique.
3. Vérifier dans l'ordre Timestamp, Snapshot, Targets et rôles délégués : version,
   expiration, seuil, longueur et hashes.
4. Ne remplacer l'état fiable local qu'après succès complet. Persister les plus
   hautes versions vues pour détecter un rollback au prochain lancement.
5. Appliquer `revocations-v1.json` et les tombstones locaux.
6. Télécharger ou lire l'archive, vérifier longueur et SHA-256 **avant** de
   l'ouvrir, puis exécuter les contrôles hostiles déjà définis par l'importeur
   Data Package.
7. Présenter l'aperçu ; ne jamais activer implicitement un nouveau paquet.

Un export hors ligne contient le dépôt TUF statique, les cibles choisies et les
racines intermédiaires requises. Le même vérificateur traite une URL, un dossier
ou cette archive : seul le transport change. Une métadonnée expirée ne devient
pas valide parce que la machine est hors ligne. Les paquets déjà validés peuvent
rester utilisables selon la politique locale, mais leur état de sécurité est
signalé comme obsolète et aucune nouvelle installation n'est présentée comme
fraîche.

Cette protection suppose une horloge système raisonnablement fiable. Sans temps
fiable ni contact avec un canal, aucun protocole local ne peut prouver qu'une
révocation plus récente n'existe pas. Le client peut mémoriser la plus haute
version et le plus grand temps déjà observés pour détecter les retours évidents
de l'horloge, mais cela ne remplace pas une source de temps fiable.

## Modèle de menace

| Menace | Protection | Limite résiduelle / règle produit |
|---|---|---|
| Modification d'un ZIP ou d'un manifeste | longueur et SHA-256 signés par Targets, puis hashes internes du Data Package | ne prouve ni qualité, ni droit de redistribution |
| Paquet arbitraire signé par la mauvaise personne | clés de rôle, seuils et délégations limitées par chemin | la politique d'enrôlement de la racine reste décisive |
| Rollback | versions TUF monotones persistées ; refus d'une version inférieure | supprimer le stockage local de confiance réinitialise cette mémoire |
| Freeze | expirations et Timestamp à courte durée | hors ligne au-delà de l'expiration : fraîcheur inconnue, donc avertissement/blocage selon l'action |
| Mix-and-match | Snapshot lie l'ensemble cohérent des métadonnées Targets | ne compare pas les vues de deux clients |
| Compromission d'une clé en ligne | rôles séparés et seuils ; Timestamp ne peut pas signer des cibles | une clé Targets autorisée compromise peut publier dans son périmètre jusqu'à révocation |
| Rotation de clés | chaîne de racines versionnées signée par anciens et nouveaux seuils | tous les intermédiaires doivent rester disponibles/importables |
| Révocation d'un paquet déjà installé | liste signée, tombstone local et quarantaine à chaque refresh | impossible de découvrir une nouvelle révocation sans état plus récent |
| Équivocation du dépôt | option de transparence et monitoring | TUF seul protège un client, pas la cohérence globale entre clients |
| Déni de service / archive hostile | limites avant téléchargement, parsing et extraction ; chemins confinés | la disponibilité parfaite n'est pas garantie |

La [spécification TUF](https://theupdateframework.github.io/specification/latest/)
vise explicitement les attaques de rollback, freeze indéfini, mix-and-match,
fast-forward et compromission partielle de clés. Snapshot empêche les mélanges de
métadonnées et Timestamp à expiration courte rend un gel observable. La racine
de confiance doit être livrée hors bande ou embarquée dans le client.

### Équivocation et transparence

TUF empêche un miroir ou un attaquant réseau de fabriquer un état incohérent,
mais un dépôt contrôlant assez de clés peut présenter deux états valides et
actuels à deux groupes de clients. Pour Answer Atlas public, une extension
optionnelle recommandée est de signer le digest de chaque Snapshot ou release
avec Sigstore et de publier l'événement dans Rekor.

Un [Sigstore Bundle](https://docs.sigstore.dev/about/bundle/) peut embarquer le
certificat, la signature, les timestamps et les éléments de preuve du journal ;
il est donc vérifiable hors ligne avec une racine Sigstore locale. Une preuve
d'inclusion montre qu'un événement a été promis ou inclus dans le journal, mais
la cohérence globale exige des monitors et du gossip. Le
[modèle de menace Sigstore](https://docs.sigstore.dev/about/threat-model/)
qualifie explicitement ce monitoring de critique.

Cette extension reste facultative pour un petit producteur : elle impose OIDC,
Fulcio/Rekor ou une autre infrastructure au moment de publier. Elle ne doit pas
empêcher la vérification TUF locale ni transformer une identité Sigstore en
éditeur approuvé sans politique explicite.

## Comparaison des options

| Option | Ce qu'elle résout | Ce qu'il faudrait encore inventer | Verdict |
|---|---|---|---|
| TUF + Data Package | index, hashes, fraîcheur, rollback, rôles, seuils, délégations, rotation ; format métier séparé | profil de canal signé, révocations de contenu et UX d'enrôlement | **Base recommandée** |
| JCS + Ed25519 | JSON déterministe et signature simple | distribution des clés, seuils, délégations, versions fiables, expiration, rollback, freeze, révocation | trop incomplet seul |
| DSSE | lie les octets exacts à un type de payload et évite la canonicalisation | PKI, identité, politique, fraîcheur et cycle de dépôt | bon format d'attestation, pas un index |
| Sigstore Bundle | identité OIDC, timestamp, preuve de transparence et vérification hors ligne | autorité sur les noms, politique de confiance, index et rollback du dépôt | complément public optionnel |
| COSE/CBOR | enveloppe signée compacte, structures normalisées | même cycle de dépôt que JCS/DSSE ; outils moins accessibles aux contributeurs JSON | futur profil binaire éventuel |
| signature seule dans `datapackage.json` | apparence de simplicité | presque tout le modèle de mise à jour et risque de règles cryptographiques ad hoc | à écarter |

[RFC 8785 JCS](https://www.rfc-editor.org/rfc/rfc8785.html) produit une
représentation JSON invariante à partir d'I-JSON, avec sérialisation des nombres
et tri déterministe des propriétés. C'est utile pour des builds reproductibles,
mais ce n'est ni une signature ni une gestion de clés. En particulier, il ne
faut pas remplacer la canonicalisation définie par l'implémentation TUF par JCS :
producteur et client doivent employer le même POUF TUF compatible.

[DSSE 1.0.2](https://github.com/secure-systems-lab/dsse/blob/master/protocol.md)
signe une pré-authentification de `payloadType` et des octets du payload ; son
`keyid` est seulement un indice non authentifié et ne doit servir à aucune
décision de sécurité. Le projet DSSE déclare la gestion de clés et la PKI hors de
son périmètre. Il ne couvre donc pas rollback ou freeze.

[COSE, RFC 9052](https://www.rfc-editor.org/rfc/rfc9052.html) définit notamment
`COSE_Sign1` pour un signataire et `COSE_Sign` pour plusieurs signataires, avec
en-têtes protégés et contenu CBOR. Son format compact est intéressant pour un
transport contraint, mais augmente la friction de lecture, de diff Git et de
contribution sans résoudre le cycle de mise à jour.

## Tranche d'implémentation recommandée

1. Publier les schémas JSON `context-target-v1` et `revocations-v1` avec des
   exemples valides/invalides et des limites explicites.
2. Ajouter au CLI `channel init`, `channel add-package`, `channel revoke`,
   `channel sign` et `channel verify`; utiliser une bibliothèque TUF conforme,
   jamais une vérification cryptographique réécrite localement.
3. Créer le canal Answer Atlas avec `consistent_snapshot`, racine officielle
   épinglée, rôles séparés et une cible `revocations-v1.json` vide.
4. Ajouter au moteur un store de confiance par canal : racine, versions maximales,
   métadonnées courantes, tombstones et dernière vérification réussie.
5. Implémenter les transports HTTPS, dossier et bundle hors ligne derrière la
   même interface, puis l'UI d'ajout/empreinte/état de fraîcheur/quarantaine.
6. Publier un kit producteur généré par le CLI et un canal exemple minimal.
7. Ajouter Sigstore uniquement après le flux TUF complet, d'abord pour les
   releases officielles et avec un monitor documenté.

La [suite officielle de conformité TUF](https://theupdateframework.github.io/tuf-conformance/)
doit guider le choix de bibliothèque. Au 2026-07-29, elle indique 108/108 tests
pour `python-tuf` et `sigstore-rust`; ce résultat doit être revérifié au moment
de figer la dépendance.

## Critères de validation

Le profil n'est prêt à être déclaré public que si des tests boîte noire couvrent :

- canal actuel valide et paquet activable seulement après aperçu ;
- signature, longueur ou hash de cible invalides ;
- rollback de chaque rôle après redémarrage du client ;
- Timestamp et Targets expirés ;
- mélange d'un Snapshot et de Targets issus de publications différentes ;
- rotation Root séquentielle avec anciens et nouveaux seuils ;
- clé déléguée tentant de publier hors de son motif de chemin ;
- révocation d'une version déjà installée et persistance du tombstone ;
- ajout d'un canal inconnu sans approbation explicite de sa racine ;
- bundle hors ligne contenant toutes les racines intermédiaires ;
- limites de taille, nombre de délégations, profondeur, archives et chemins ;
- affichage exact de l'âge de l'état quand aucun refresh n'est possible ;
- constat documenté que l'équivocation globale n'est pas détectable hors ligne
  sans preuve et monitoring externes.

## Sources primaires

- [The Update Framework — spécification courante](https://theupdateframework.github.io/specification/latest/)
- [TUF — rôles et métadonnées](https://theupdateframework.io/docs/metadata/)
- [TUF — spécification source et format `custom`](https://github.com/theupdateframework/specification/blob/master/tuf-spec.md)
- [TUF — conformité des clients](https://theupdateframework.github.io/tuf-conformance/)
- [Data Package Standard 2.0](https://datapackage.org/standard/data-package/)
- [Data Resource Standard 2.0](https://datapackage.org/standard/data-resource/)
- [DSSE — protocole 1.0.2](https://github.com/secure-systems-lab/dsse/blob/master/protocol.md)
- [DSSE — enveloppe JSON 1.0.2](https://github.com/secure-systems-lab/dsse/blob/master/envelope.md)
- [Sigstore — Bundle Format](https://docs.sigstore.dev/about/bundle/)
- [Sigstore — Threat Model](https://docs.sigstore.dev/about/threat-model/)
- [RFC 8785 — JSON Canonicalization Scheme](https://www.rfc-editor.org/rfc/rfc8785.html)
- [RFC 9052 — CBOR Object Signing and Encryption](https://www.rfc-editor.org/rfc/rfc9052.html)
- [RFC 8032 — Ed25519](https://www.rfc-editor.org/rfc/rfc8032.html)
