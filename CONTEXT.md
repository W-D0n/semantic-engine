# Semantic Recognition

Ce contexte décrit le langage du produit : recevoir une expression humaine
imparfaite et lui associer, ou non, un sens configuré.

## Language

**Source d’entrée**:
Origine externe qui fournit des messages : terminal, webhook, Twitch ou YouTube.
_Avoid_: Chat, plateforme

**Message brut**:
Contenu reçu d’une source avant validation et normalisation.
_Avoid_: Prompt, requête

**Énoncé**:
Texte validé et normalisé que le moteur doit interpréter.
_Avoid_: Input, phrase nettoyée

**Contexte de reconnaissance**:
Ensemble versionné des interprétations actives, expressions et politiques d’un usage.
_Avoid_: Dictionnaire, namespace

**Paquet de contexte**:
Unité importable et diffusable contenant un contexte, ses métadonnées, sa licence,
ses schémas et ses empreintes d’intégrité.
_Avoid_: Dictionnaire global, dump

**Inspection de paquet**:
Vérification locale du format, des limites, de la licence et des empreintes d’un paquet,
sans modifier le contexte actif.
_Avoid_: Import, activation

**Activation de contexte**:
Action explicite et atomique qui rend actif le paquet exactement inspecté.
_Avoid_: Import, sélection de fichier

**Contexte actif**:
Version persistée du contexte de reconnaissance utilisée par les workflows locaux.
_Avoid_: Dernier paquet ouvert, cible du round

**Brouillon local de cible**:
Calque opérateur persistant qui ajuste le canonique ou les alias d’une cible sans
modifier la version publiée du paquet.
_Avoid_: Nouvelle version, apprentissage, mutation du paquet

**Restauration de contexte**:
Activation atomique de la version qui précédait directement le contexte actif.
_Avoid_: Annulation de fichier, arbitrage, rollback

**Interprétation**:
Sens métier stable que le moteur peut attribuer à un énoncé.
_Avoid_: Intent, réponse, label

**Expression connue**:
Formulation canonique, alias ou abréviation rattachée à une interprétation.
_Avoid_: Mot-clé, synonyme

**Résultat de reconnaissance**:
Interprétation acceptée ou abstention, avec confiance et indices vérifiables.
_Avoid_: Prédiction, réponse

**Abstention**:
Décision de ne rien choisir quand la confiance ou l’écart entre candidats est insuffisant.
_Avoid_: Erreur, inconnu

**Correction**:
Retour validé par un opérateur indiquant l’interprétation attendue.
_Avoid_: Feedback, apprentissage automatique

**Mémoire de reconnaissance**:
Exemples validés et résultats réutilisables, versionnés avec le contexte.
_Avoid_: Cache, historique du chat

**Connexion de source**:
Configuration autorisée reliant une source à un contexte de reconnaissance.
_Avoid_: Intégration, bot

**Opérateur**:
Personne autorisée à configurer, corriger ou gérer une connexion.
_Avoid_: Admin, streamer, propriétaire

**Round**:
Fenêtre de jeu identifiée pendant laquelle une ou plusieurs cibles sont acceptables.
_Avoid_: Partie, question

**Cible**:
Œuvre ou sens attendu pour un round, avec titre canonique et alias acceptés.
_Avoid_: Bonne chaîne, mot-clé

**Soumission**:
Réponse d’un participant, identifiée et ordonnée par sa source.
_Avoid_: Proposition, message seul

**Validation**:
Résultat déterministe indiquant si une soumission correspond à une cible.
_Avoid_: Victoire, point

**Décision opérateur**:
Acceptation ou rejet manuel rattaché à une validation, conservant la décision
moteur originale et l’identité ordonnée de la soumission.
_Avoid_: Réécriture du résultat, apprentissage automatique, score

**Acceptation**:
Validation positive qu’un workflow externe peut consommer de façon idempotente.
_Avoid_: Gagnant, score

**Arbitrage**:
Sélection externe de la première acceptation d’un round selon l’ordre de source.
_Avoid_: Reconnaissance, validation
