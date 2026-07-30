# Conventions des paquets de contexte

## Décision

Un « dictionnaire » Semantic Engine est publié comme un **paquet de contexte** :
un Data Package v2 étendu par un profil métier JSON Schema. Cette base existante
évite d’inventer le manifeste, la licence, la provenance et la liste de ressources.

Le standard Data Package a précisément pour objectif la livraison, l’installation
et la gestion commodes de jeux de données. Son descripteur public s’appelle
`datapackage.json`, reste à la racine et référence une ou plusieurs ressources.

## Références retenues

| Besoin | Convention | Usage dans le projet |
|---|---|---|
| conteneur | [Data Package v2](https://datapackage.org/standard/data-package/) | manifeste, identité, ressources, sources, licence |
| extension métier | [profils Data Package](https://datapackage.org/standard/extensions/) | `$schema` et bloc `semanticEngine` |
| validation | [JSON Schema 2020-12](https://json-schema.org/specification) | profil du paquet et schéma des titres |
| compatibilité | [Semantic Versioning 2.0.0](https://semver.org/) | version du paquet et du format |
| licence | [expressions SPDX](https://spdx.github.io/spdx-spec/v2.2.2/SPDX-license-expressions/) | identifiant de licence lisible par machine |
| intégrité | SHA-256 | taille et empreinte de chaque ressource |
| signature future | [RFC 8785 JCS](https://www.ietf.org/rfc/rfc8785.html) | canonisation JSON avant signature |

Le profil Semantic Engine est une extension, pas un fork de Data Package. Un
outil générique peut donc lire les métadonnées usuelles ; un importeur Semantic
Engine applique en plus les contraintes de reconnaissance.

## Règles de compatibilité

- `id` et `name` restent stables entre versions ;
- `version` est un SemVer et une version publiée ne change jamais de contenu ;
- `PATCH` corrige les métadonnées ou alias sans changer le sens attendu ;
- `MINOR` ajoute des cibles ou des champs compatibles ;
- `MAJOR` change la structure ou le sens de données existantes ;
- `semanticEngine.formatVersion` versionne séparément le protocole d’import ;
- chaque ressource déclare ses octets et son `sha256:` ;
- la licence des données est distincte de la licence du logiciel ;
- sources et provenance sont renseignées avant publication.

Une archive de distribution conserve `datapackage.json` à sa racine. Un registre
n’est pas requis : une release Git, un stockage objet ou un site HTTPS statique
suffisent. Un futur catalogue ne contiendra que métadonnées, URL, version et hash.

## Sécurité de l’import

L’importeur ne fait jamais confiance au paquet. Il traite le manifeste comme une
entrée hostile, impose des tailles maximales, refuse chemins absolus et traversées
`..`, vérifie taille et SHA-256 avant désérialisation, borne cibles/alias/chaînes,
puis charge le contexte sans l’activer implicitement.

Trois niveaux de confiance sont prévus :

1. **local** : schémas et limites seulement ;
2. **intègre** : hash publié sur un canal indépendant ;
3. **authentifié** : signature d’un éditeur approuvé, à implémenter.

Une empreinte prouve que les octets n’ont pas changé ; elle ne prouve ni l’auteur,
ni la qualité, ni les droits. L’interface devra donc afficher éditeur, sources,
licence, version, nombre de cibles et avertissements avant activation.

## Alternatives écartées

- format JSON ad hoc : simple au début, mais incompatible avec l’écosystème data ;
- dictionnaire linguistique global : trop large et peu pertinent pour des titres ;
- paquet de code/npm/crate : mélange données non fiables et exécution ;
- base SQLite brute : difficile à relire, comparer et fusionner publiquement ;
- vecteurs embarqués obligatoires : lourds, dépendants d’un modèle et inutiles au MVP.

Le JSON de titres reste volontairement petit et auditable. Des ressources
optionnelles (benchmarks, traductions, embeddings liés à un modèle) pourront être
ajoutées sans modifier la ressource de base.
