---
title: "Recursos e status"
description: "O que é suportado, experimental, planejado ou sem suporte, recurso por recurso."
group: "Start"
order: 3
source: "README.md"
source_hash: "294334626649742f3eb37a4cbc521eaf6a9e12ad6cad3e286b97072327f0bd66"
---

**Suportado** significa: implementado, e coberto por testes automatizados que executam o daemon em um compositor (um sway headless, no CI). **Experimental** significa que funciona, com uma limitação que convém conhecer, ou que ainda não foi verificado onde importa. Os recursos **planejados** vêm depois da 1.0.

| Recurso | Status | Notas |
|---|---|---|
| Imagens: PNG, JPEG, WebP, BMP, TIFF, GIF e os demais formatos do crate [`image`](https://crates.io/crates/image) | Suportado | Desenhadas na resolução física do monitor, e a cópia decodificada é liberada em seguida. Imagens com mais de 16384 pixels por lado, ou que precisariam de mais de 512 MiB, são recusadas. |
| Vídeos, em loop, sem áudio | Suportado | Qualquer formato que os plugins GStreamer instalados reproduzam. Verificados: H.264, HEVC, VP8, VP9 e AV1, e dez contêineres (MP4, WebM, Matroska, QuickTime, AVI, Ogg, ...). |
| Escala: `fill`, `fit`, `stretch`, `center`, `tile` e uma cor de fundo | Suportado | Para imagens e vídeos (`tile` age como `center` para um vídeo). |
| HiDPI e escala fracionária | Suportado | Verificado pixel a pixel nas escalas 1, 1.25, 1.5 e 2. |
| Todos os monitores, ou os que forem indicados; um papel de parede por monitor; monitores conectados depois | Suportado | Com imagens. Com vídeo, veja a próxima linha. |
| Mudanças de monitor durante a reprodução de um vídeo | Experimental | Um monitor conectado, desconectado, reescalado, movido ou ligado deixa os vídeos tocando (verificado em um sway headless; até a 1.0.0-beta.1 isso derrubava o daemon dentro do GStreamer, [problema conhecido 1](/apps/veil/docs/known-issues)). Ainda não verificado no Hyprland. |
| `veil set`, `clear`, `pause`, `resume`, `toggle`, `status`, `outputs` (também como JSON), `quit`, `ping` | Suportado | Códigos de saída estáveis. Um socket de controle para scripts e barras de status ([`docs/ipc.md`](/apps/veil/docs/ipc)). |
| Um novo papel de parede só substitui o anterior quando está pronto | Suportado | Um vídeo assume quando o primeiro quadro está na tela; um arquivo que não pode ser exibido deixa o papel de parede anterior como está. |
| Papéis de parede salvos e exibidos de novo após uma reinicialização | Suportado | Por monitor, em `~/.local/state/veil/state.json`. |
| Arquivo de configuração | Suportado | `~/.config/veil/config.toml` ([`docs/configuration.md`](/apps/veil/docs/configuration)). |
| Unidade de usuário do systemd | Suportado | Pronta quando todos os monitores exibem seu papel de parede, reiniciada após uma falha, com watchdog. |
| Pausar um vídeo atrás de uma janela em tela cheia (ou de janelas que o cubram) | Suportado no sway, experimental no Hyprland | Lido do IPC do compositor. No Hyprland 0.56.2, uma janela em tela cheia pausou a sua saída e, ao fechá-la, o vídeo voltou a ser reproduzido (2026-10-20); com `--pause-on covered` e nos demais casos, a verificação foi feita apenas contra um Hyprland falso. |
| Interromper um vídeo enquanto a tela está bloqueada ou o monitor está desligado | Suportado no sway, experimental nos demais | Funciona em qualquer compositor que pare de aceitar os quadros do vídeo, como o sway; não verificado no Hyprland (bloqueio e DPMS). O vídeo recomeça do início. |
| Decodificação de vídeo por hardware | Experimental | NVIDIA (`nvcodec`) decodifica na GPU, mas cada quadro é convertido para RGB na CPU ([problema conhecido 7](/apps/veil/docs/known-issues)). Intel e AMD (VA-API) decodificam na GPU e entregam seus quadros ao compositor como estão, em dmabufs, quando o compositor os aceita (o sway aceita; o Hyprland até a 0.57 não, então lá eles também são convertidos na CPU). Verificado em Intel em um sway headless; AMD e uma sessão real não foram verificados. A decodificação por software sempre funciona. |
| GIFs animados | Suportado | Reproduzidos como vídeos; exige os plugins `libav` do GStreamer. |
| Hyprland | Experimental | O alvo principal do veil e a nossa configuração do dia a dia (0.56.2 e 0.57.2). O backend dele (a pausa automática) é testado contra um Hyprland falso; a renderização do próprio Hyprland não é coberta por testes ([`docs/testing.md`](https://github.com/ordreth/veil/blob/stable/docs/testing.md)). |
| Outros compositores com layer-shell (river, niri, Wayfire, labwc, KDE Plasma) | Experimental | Deve funcionar; ninguém relatou ainda. Sem pausa atrás de janelas nesses compositores (a interrupção por bloqueio e desligamento continua valendo). |
| Autocompletar do shell (bash, zsh, fish, nushell) e páginas de manual | Suportado | Gerados pelo próprio binário. |
| Flake e pacote do Nix | Suportado | O pacote traz seus plugins do GStreamer. |
| Binários pré-compilados para Linux x86_64 e aarch64 | Experimental | Veja [Binários pré-compilados](/apps/veil/docs/install#binários-pré-compilados); publicados a cada lançamento. O tarball x86_64 foi executado no Debian 12 e 13, no Ubuntu 22.04 e 24.04, no Fedora 44 e no Arch; o aarch64 não foi executado. Com GStreamer anterior à 1.26, GIFs animados não são reproduzidos (problema conhecido 8). |
| GNOME, X11 | Não | O GNOME não tem layer-shell. |
| HEIF/HEIC, AVIF, JPEG XL; WebP e PNG animados | Não | Recusados com o motivo; um WebP ou PNG animado mostra o seu primeiro quadro. |
| Áudio, limite de taxa de quadros, velocidade de reprodução, pontos de loop | Planejado | Depois da 1.0. |
| Playlists, rotação de diretório, transições | Planejado | Depois da 1.0. `veil set` a partir de um script ou de um timer funciona hoje. |
| Pausar quando ocioso ou na bateria; pausar atrás de janelas no niri e no river | Planejado | Depois da 1.0. |
| Módulos do NixOS e do home-manager, AUR, pacotes `.deb` e `.rpm` | Planejado | Depois da 1.0. |
