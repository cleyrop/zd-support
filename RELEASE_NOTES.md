Première release publique de **zd-support**, version 0.3.6.

- Authentification par compte support Zendesk ou OAuth PKCE.
- Création, consultation, réponse et résolution des demandes autorisées.
- Conversations avec noms des auteurs, couleurs et cadres adaptés au terminal.
- Sortie JSON pour les agents et scripts ; surveillance avec `watch`.
- Protection des fichiers de session et absence de renvoi automatique des écritures.

Archives Linux x86_64/ARM64 et macOS Intel/Apple Silicon. Les binaires macOS sont
signés Developer ID et notarisés par Apple. Les archives finales et SHA256SUMS
disposent d’une attestation de provenance GitHub.

Vérifier une archive téléchargée avant installation :

```sh
gh attestation verify ARCHIVE --repo cleyrop/zd-support
```

Le mot de passe ou les jetons restent stockés en clair dans un fichier privé
sur le poste utilisateur. Voir le README pour les limites et les commandes.
