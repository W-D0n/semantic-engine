# Continuité entre sessions et outils IA

Une reprise ne doit pas dépendre de l’historique d’un chat.

## Lire dans cet ordre

1. [Accueil du guide](../index.md)
2. [Langage du domaine](../domain-language.md)
3. [Roadmap](../roadmap.md)
4. les ADR listés dans la navigation
5. statut Git, tests et dernière release

Une session vise un seul résultat vérifiable. Vérifier les changements existants,
préférer un tracer bullet aux squelettes, mettre à jour glossaire/ADR/tests au fil
du travail, puis terminer avec preuves, limites et prochaine action.

## Boucle des erreurs connues

Après les tests et avant le handoff final, consulter
`C:\DEV\error-tracking\README.md` :

1. rechercher d’abord symptôme, outil et cause avec `rg` ;
2. compléter une fiche existante plutôt que dupliquer ;
3. ajouter une fiche seulement si l’enseignement est coûteux ou réutilisable ;
4. conserver contexte, symptôme, cause, résolution et prévention vérifiable ;
5. exécuter la validation minimale inscrite dans ce registre ;
6. garder son commit séparé de celui de Semantic Engine.

Le registre vivant est relu juste avant écriture afin de fusionner les changements
concurrents. Son indisponibilité ne bloque pas le livrable : le handoff note alors
précisément la connaissance restant à publier.

## Modèle portable

```markdown
# Handoff — Semantic Engine
## Objectif de la prochaine session
## État : branche, commit, jalon, terminé/non terminé
## Lire d’abord : chemins précis
## Décisions et contraintes non déjà documentées
## Vérification : commandes, résultats, risques
## Première action recommandée
## Compétences suggérées
```

Markdown et Mermaid restent la source de vérité. Aucun secret, donnée personnelle
ou état essentiel ne doit vivre seulement dans une mémoire d’agent.
