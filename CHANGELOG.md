# Changelog

## 0.4.0 — 2026-11-02

- Ajout de `version` et de `self-update`, avec `--check` et sortie JSON.
- Mise à jour autonome : SHA-256 via HTTPS, contrôle Developer ID sur macOS, remplacement atomique.
- Réduction mesurée de 44,13 % du binaire macOS ARM64, nouvelles commandes incluses.
- Suppression du module terminal redondant et conservation du comportement réseau existant.

## 0.3.6 — 2026-11-02

Première version publique ; les versions précédentes correspondaient au développement local.

- Authentification Zendesk par mot de passe ou OAuth PKCE.
- Liste filtrée et triée, fiche de ticket, conversations publiques et sortie JSON.
- Création, réponse, résolution lorsque permise et surveillance des demandes.
- Sessions privées, écritures sans nouvelle tentative automatique, validation des réponses API.
- CI pour Linux et macOS, archives signées/notarisées sur macOS et provenance GitHub.

## 0.3.5 — tentative non publiée

La signature Apple a bloqué la publication. La version 0.3.6 corrige la découverte
du trousseau de signature et vérifie son nettoyage en cas d’échec.
