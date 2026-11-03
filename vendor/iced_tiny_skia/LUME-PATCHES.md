# Patch Lume per iced_tiny_skia 0.14.0

Fonte: pacchetto crates.io 0.14.0, SHA256 archivio `fe0acf8b75a3bc914aff5f2329fdffc1b36eeaea29dda0e4bd232f1c62e9cc3d`.
Licenza MIT come dichiarata dal pacchetto; verifiche distribuzione nel gate finale.

Patch iniziale in src/engine.rs: colore uniforme, radii zero,
bounds fisici finiti/interi usano fill_rect sul danno arrotondato verso
l'esterno, mantenendo mask originale. Gli altri casi conservano fill_path.
Ombre/bordi/testo invariati. Nessun timer diagnostico nella produzione.

Test con draw_quad upstream oracle:120 casi scala/offset/clipping/stili,
70 confronti della funzione e una diagnostica ignored. Prima di aggiornare
upstream rivalidare patch,oracle,fallback e misure. La ricevuta prototipo
renderer-rectangle-app-20261011.md non certifica questa release ordinaria.

Integrazione workspace: rustfmt del repository e quattro eccezioni mirate
Clippy too_many_arguments sulle firme upstream mantenute di text::draw,
Pipeline::draw_cached, raster::Pipeline::draw e vector::Pipeline::draw.
LICENSE locale mantiene il checksum MIT già registrato in about.toml;
la chiarifica usa il file vendorizzato.


Correzione redraw testo (2026-10-11): Layer::damage richiede che due primitive
Paragraph, oltre alle proprietà geometriche/colore uguali, condividano il
buffer shaped. PartialEq upstream dei Paragraph confronta metriche senza
contenuto; numeri diversi della stessa larghezza potevano lasciare pixel
obsoleti. Identità buffer verificata a costo costante tramite Weak upgrade,
nessuna scansione del testo e nessun repaint globale. Clone invariato mantiene
il percorso senza danno. Oracle scroll partial/full RED prima, GREEN dopo;
qualificazione GUI/release e gate completi da rinnovare.


Quad frazionari: maschera usata anche quando i bounds sono contenuti nel
clip. Mantiene il percorso raster tra fullframe e danno parziale, evitando
residui di background ai bordi delle righe. Quad interi e fastpath invariati.
Oracle con background perriga, due metriche, quattro scale e tre transizioni
confronta pixel esatti; gateGUI/prestazioni ancora obbligatori.

Qualificazione Linux del fix finale: build release, Clippy, MSRV1.89,
fmt e test workspace terminati exit0. GUI su corpus31MiB:31PageDown,
gutter partial/full identico e file invariato;31campioni typing e save
byte-esatto. Mediana4.731ms/p956.367ms include X11/polling, CPU non isolata.
Le ricevute e i sorgenti identificati sono conservati fuori dal pacchetto.
Matrice feature rinnovata in corso; budget Windows e60fps restano aperti.
