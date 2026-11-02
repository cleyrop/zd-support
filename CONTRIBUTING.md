# Contribuer

Utiliser des commits atomiques au format Conventional Commits (`feat`, `fix`,
`docs`, `test`, `ci`, `chore`), avec un scope si utile. Une modification doit
inclure ses tests pertinents et sa documentation.

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo deny --locked check advisories licenses sources
python3 -m unittest discover -s scripts -p 'test_*.py' -v
go run github.com/rhysd/actionlint/cmd/actionlint@v1.7.12 -shellcheck=
```

Les tests HTTP utilisent exclusivement des serveurs locaux et des identifiants fictifs.
Ne jamais ajouter une session Zendesk, un certificat privé ou une clé au dépôt.

## Publier

1. Mettre à jour `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md` et `RELEASE_NOTES.md`.
2. Faire intégrer les changements sur `main`, avec une CI verte.
3. Créer un tag signé correspondant exactement à la version, puis le pousser :
   `git tag -s vX.Y.Z -m 'release: vX.Y.Z'` et `git push origin vX.Y.Z`.
4. Suivre le workflow Release. Une exécution manuelle est possible uniquement
   en sélectionnant ce tag, pas la branche `main`.

La release vérifie que le commit est dans `main`, réexécute la CI, compile les
quatre plateformes, signe et notarise les binaires macOS, calcule SHA256SUMS,
atteste les fichiers finaux et publie une release GitHub après chargement des actifs.
Les actions sont épinglées sur des SHA. Les jobs sans publication ont un token
en lecture seule. Le job de signature utilise un trousseau temporaire supprimé
dans un bloc de nettoyage, puis le runner GitHub est détruit.

Les cinq secrets `APPLE_*` sont fournis par l’organisation. L’environnement
`release` n’autorise que les tags `v*`. Les signatures Apple identifient le
titulaire du certificat fourni ; la provenance GitHub identifie le dépôt et le workflow.
Un binaire CLI autonome dans un ZIP ne reçoit pas de ticket staplé : sa
notarisation est enregistrée auprès d’Apple et peut nécessiter une connexion réseau.

Une release déjà publiée ne doit jamais être modifiée ni son tag déplacé.
En cas d’échec avant publication, examiner l’exécution et l’éventuel brouillon
avant toute nouvelle tentative. Le workflow refuse de remplacer une release existante.
