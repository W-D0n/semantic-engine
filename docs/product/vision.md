# Vision produit

## Problème

Dans un chat réel, une même réponse arrive avec une casse différente, des accents
oubliés, des fautes, initiales, surnoms ou paraphrases. L’égalité de chaînes est
trop fragile ; un LLM généraliste est souvent trop coûteux et peu explicable.

## Produit

Le produit cible reçoit un message et un contexte versionné. Il retourne une
interprétation ou une abstention, une confiance calibrée, les meilleurs candidats,
des indices vérifiables et les versions utilisées. L’application décide ensuite
quoi faire : le moteur ne publie ni ne sanctionne de lui-même. Le vertical slice
actuel expose décision, cible, confiance, preuve et erreur typée ; alternatives,
versions et latence cœur restent à ajouter au contrat public.

## Utilisateur prioritaire

Un créateur d’expérience interactive en direct veut reconnaître un petit ensemble
de réponses sans infrastructure ML complexe. Il doit pouvoir créer un contexte,
tester des exemples, connecter une source, corriger une erreur puis exporter ses
données sans coder.

## Exigences

| Priorité | Exigence | Preuve |
|---|---|---|
| P0 | installation locale simple | une commande ou une image conteneur |
| P0 | configuration sans code | fichier déclaratif puis interface |
| P0 | décision explicable | confiance, candidats et indices |
| P0 | abstention fiable | seuil et marge testés |
| P0 | moteur séparé des sources | mêmes tests avec plusieurs adaptateurs |
| P0 | données maîtrisées | stockage local, export, suppression |
| P1 | mémoire dynamique sûre | corrections validées et révocables |
| P1 | Twitch et YouTube | OAuth, réception et révocation |
| P1 | auth opérateur | moindre privilège et secrets isolés |

## Mesures

Mesurer la précision des décisions acceptées, la couverture, les faux positifs,
les abstentions, la latence p50/p95, la mémoire et les corrections nécessaires.
L’objectif initial à valider sur corpus est au moins 95 % de précision acceptée ;
la couverture ne progresse qu’après protection de cette précision.

> Une reconnaissance locale, configurable et expliquée, assez prudente pour dire
> « je ne sais pas ».
