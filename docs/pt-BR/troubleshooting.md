---
title: "Solução de problemas"
description: "Falhas comuns e suas mensagens, um guia rápido de depuração e a tabela de compatibilidade."
group: "Reference"
order: 8
source: "docs/troubleshooting.md"
source_hash: "9b762c09566135e26cc308934ccac650c3826c3d26b03cbf99d888e7a41256c3"
---

O que fazer quando o veil não mostra o que deveria: por onde começar, as falhas comuns o bastante para terem nome, um guia rápido dos comandos que dizem mais e uma tabela dos compositores e placas gráficas em que o veil foi testado.

As falhas abaixo foram produzidas de propósito em um sway 1.12 headless com GStreamer 1.28.7, e as mensagens são as que o daemon deu (2026-10-19; como, em [`manual-tests.md`](https://github.com/ordreth/veil/blob/main/docs/manual-tests.md), "Troubleshooting"); a tabela de outras mensagens cita o código. As linhas de log são mostradas sem o timestamp. As limitações que o veil conhece e não consegue corrigir hoje, como a falha em uma mudança de monitor enquanto um vídeo é reproduzido, estão em [`known-issues.md`](/apps/veil/docs/known-issues).

## Por onde começar

1. **Há um daemon rodando?** `veil ping` imprime `pong`, ou termina com código de saída 3 quando nenhum daemon roda para este compositor. Sob o systemd: `systemctl --user status veil` (ele também diz como foi a inicialização).
2. **O que ele acha que está exibindo?** `veil status` tem uma linha por saída:

   ```text
   veil 1.0.0-beta.1 (pid 295816), running for 4s; videos pause on: fullscreen
   default: ~/walls/sea.mp4 (fill, #000000)
   HEADLESS-1: playing ~/walls/sea.mp4 (fill, #000000); 1920x1080 at scale 1; video 1920x1080, 30 fps, 71 frames shown, 0 dropped
   ```

   A palavra depois do nome da saída é o seu estado:

   | Estado | Significado |
   |---|---|
   | `showing` | Uma imagem, ou a cor de fundo, está na tela. |
   | `starting` | Um vídeo foi carregado e espera pelo primeiro quadro (no máximo 10 segundos). |
   | `playing` | Um vídeo é reproduzido. As contagens de quadros são as do próprio sink de vídeo; elas recomeçam a cada repetição. |
   | `paused (by veil pause)`, `paused (hidden by a window)` | Um vídeo está pausado, à mão ou porque uma janela em tela cheia (ou, com `--pause-on covered`, as janelas) o esconde. |
   | `stopped while the compositor does not show it` | O compositor não aceitou nenhum quadro do vídeo por 2 segundos (uma tela bloqueada, um monitor desligado, no sway uma janela sobre toda a saída), então o veil soltou o vídeo. Ele recomeça do início quando o compositor volta a desenhar a saída. |
   | `stopped after an error (see the log)` | O vídeo não pôde ser iniciado, ou falhou e não pôde ser reiniciado. A superfície mostra a cor de fundo do vídeo. O motivo está no log. |
   | `not connected` | A saída está desconectada ou desativada. Seu papel de parede volta com ela. |

   Uma saída que falta na lista é uma em que o daemon não desenha (veja [Um nome de saída não é encontrado](#um-nome-de-saída-não-é-encontrado)).
3. **O que diz o log?** `journalctl --user -u veil` sob o systemd, ou então o stderr do daemon. A primeira linha informa as versões (`veil 1.0.0-beta.1 (GStreamer 1.28.7)`), e todo vídeo registra o decodificador que o GStreamer escolheu para ele (`video decoder: nvh264dec (hardware)`).

## A tela está preta ou mostra apenas uma cor

`veil status` distingue os casos:

| `veil status` diz | Por quê | O que fazer |
|---|---|---|
| `veil ping` termina com código de saída 3 | Nenhum daemon roda. | Iniciá-lo (`veil daemon`, ou `systemctl --user start veil`). Sob o systemd, `systemctl --user status veil` e o journal dizem por que ele parou. |
| A saída não está na lista | O daemon não desenha nela: `--output` (ou `outputs` no arquivo de configuração) nomeia outras saídas, ou um nome está escrito errado. O que se vê é o fundo do próprio compositor. | [Um nome de saída não é encontrado](#um-nome-de-saída-não-é-encontrado). |
| `showing the background colour #000000` | Nada foi definido: nenhum `--input`, nada no arquivo de configuração, nada salvo, ou `veil clear`. Preto é o `--bg-color` padrão. | `veil set --input FILE`. |
| `starting ...` | Um vídeo espera pelo primeiro quadro. Um disco lento ou um decodificador que está sendo carregado pode demorar um pouco; após 10 segundos, conta como falho. | Esperar; depois ver o log. |
| `stopped after an error (see the log)` | O vídeo não pode ser reproduzido. | A linha de log `Failed to show the wallpaper on ...` diz por quê; os motivos mais comuns estão abaixo ([Um vídeo não é reproduzido](#um-vídeo-não-é-reproduzido-falta-um-elemento-do-gstreamer)). |
| `stopped while the compositor does not show it` | Ninguém está vendo o vídeo agora. | Nada: ele volta a ser reproduzido quando a saída for desenhada de novo. `--release-unseen-after 0` desativa isso. |
| `showing` ou `playing`, e as contagens de quadros sobem, mas outra coisa está na tela | Outro programa desenha na mesma camada: outro daemon de papel de parede (`swaybg`, `hyprpaper`, `swww`, `mpvpaper`, ...). No sway, o iniciado por último fica por cima, e um `veil set` posterior não muda isso (visto com o `swaybg` 1.2.2). | Parar o outro programa, ou removê-lo da configuração do compositor (o sway inicia o `swaybg` para uma linha `output * bg ...`). |

**Para desenvolvedores: um vídeo precisa de um buffer sob ele.** O `waylandsink` desenha o vídeo em uma subsuperfície da superfície do veil, e uma subsuperfície só é exibida quando sua superfície pai está mapeada, isto é, tem um buffer anexado e confirmado por commit. O veil põe a cor de fundo do vídeo sob todo vídeo (`RenderTarget::fill`). Uma compilação que deixa o buffer de fora mostra o fundo do compositor onde o vídeo deveria estar: testado com as três chamadas de `fill` de `engine/video.rs` removidas, a tela mostrou a própria cor de fundo do sway, nada se mexeu, o sink contou todo quadro como descartado, e dois segundos depois `veil status` disse `stopped while the compositor does not show it`.

## Um vídeo não é reproduzido: falta um elemento do GStreamer

O veil monta todo pipeline de vídeo com elementos do GStreamer encontrados em tempo de execução. Se um estiver ausente, o vídeo não pode ser exibido:

| O log ou o `veil set` diz | Ausente | No conjunto de plugins |
|---|---|---|
| `Internal GLib/GStreamer error: no element "waylandsink"` | o sink de vídeo | `gst-plugins-bad` |
| `Internal GLib/GStreamer error: no element "videoconvert"` | o conversor | `gst-plugins-base` (ou [um registro desatualizado](#um-registro-do-gstreamer-desatualizado), ou [plugins de outro GStreamer](#plugins-de-outra-versão-do-gstreamer)) |
| `Internal GLib/GStreamer error: no element "videocrop"` | o recorte do `fill` | `gst-plugins-good` |
| `GStreamer pipeline error: the video cannot be played: Your GStreamer installation is missing a plug-in.`, após um aviso `No decoder available for type 'video/webm'` | um demuxer ou um decodificador para este arquivo | veja abaixo |

O tipo no aviso diz qual: `video/webm` e `video/x-matroska` precisam do `matroskademux`, `video/quicktime` (MP4, MOV) do `qtdemux`, ambos do `gst-plugins-good`; VP8 e VP9 são decodificados pelo `gst-plugins-good` (`vp8dec`, `vp9dec`), H.264 e HEVC por software pelo `gst-libav` (`avdec_h264`, `avdec_h265`); `image/gif` (um GIF animado) precisa do `gst-libav` (`avdemux_gif`, `avdec_gif`). Os decodificadores por hardware vêm do `gst-plugins-bad` (`va`, `nvcodec`) e precisam de um driver (veja [Sem decodificação por hardware](#sem-decodificação-por-hardware)). Os nomes dos pacotes no Arch, Debian, Ubuntu e Fedora estão no README ([A partir do código-fonte](/apps/veil/docs/install#a-partir-do-código-fonte)); o `gst-libav` do próprio Fedora não tem decodificador H.264 nem HEVC (o `openh264dec` decodifica H.264 lá).

O que acontece então:

* `veil set --input FILE`: ele imprime o erro e termina com código de saída 1; o papel de parede na tela permanece.
* `veil daemon --input FILE`: a falta do `waylandsink`, do `videoconvert` ou do `videocrop` para o daemon (código de saída 1, com uma notificação de desktop). A falta de um demuxer ou de um decodificador não: o daemon roda, a saída mostra a cor de fundo e `veil status` diz `stopped after an error`.
* As imagens não passam pelo GStreamer: um daemon sem o `gst-plugins-bad` exibiu uma imagem normalmente.

**Com o pacote Nix**, tudo isso já vem nele (`base`, `good`, `bad`, `libav`; a verificação `plugins` do pacote procura todo elemento que o veil usa). Um elemento ausente ali significa um registro desatualizado ou plugins de outro GStreamer em `GST_PLUGIN_PATH` (as duas próximas seções).

**Em outras distribuições**, instale os conjuntos de plugins. Os pacotes costumam ter o nome deles: `gst-plugins-bad` (Arch), `gstreamer1.0-plugins-bad` (Debian, Ubuntu), `gstreamer1-plugins-bad-free` (Fedora), e o mesmo para `base`, `good` e `libav` (não verificado nessas distribuições). Verifique com a ferramenta do próprio GStreamer, que enxerga os mesmos plugins que o veil:

```bash
gst-inspect-1.0 waylandsink videoconvert videocrop > /dev/null && echo "all there"
gst-inspect-1.0 --exists avdec_h264 || echo "no software H.264 decoder"
```

Dentro do dev shell Nix do veil, o `gst-inspect-1.0` pode ser o do sistema e listar também os plugins do sistema; isso não é uma verificação do pacote.

## Um GIF animado fica parado, não inicia ou mantém um núcleo ocupado

Com o GStreamer anterior à 1.26, o demuxer de GIF do GStreamer falha (problema conhecido 8; visto com o tarball de lançamento): no Ubuntu 22.04 e no Debian 12 o GIF fica parado em um de seus primeiros quadros enquanto `veil status` diz `playing`, e no Debian 12 o daemon também usa um núcleo inteiro da CPU; no Ubuntu 24.04, `veil set` falha com `the video did not show a frame within 10s`. Imagens e outros vídeos não são afetados, nem o pacote Nix, que traz o GStreamer 1.28. Converta o GIF para um vídeo, que é reproduzido em todos eles:

```bash
ffmpeg -i anim.gif -c:v libvpx-vp9 -pix_fmt yuv420p -vf 'scale=trunc(iw/2)*2:trunc(ih/2)*2' anim.webm
veil set --input anim.webm
```

(O `scale` torna a largura e a altura pares, o que o formato de cor exige.)

## Um registro do GStreamer desatualizado

O GStreamer mantém um cache dos plugins que encontrou, o *registro*. Um plugin que falhou ao carregar uma vez é registrado como falho e deixado de fora enquanto seu arquivo mantiver o caminho, a data de modificação e o tamanho, mesmo que o motivo tenha desaparecido. No Nix store os arquivos nunca mudam, então ele fica de fora para sempre. Como o `nvcodec` não declara dependência do driver, o mesmo vale para os decodificadores da NVIDIA: um registro escrito quando o driver não podia ser alcançado os mantém de fora depois que o driver volta a funcionar (testado: escrito com `CUDA_VISIBLE_DEVICES=` definido, a próxima inicialização sem ele ainda decodificou por software). O plugin `va` é examinado de novo quando os dispositivos de renderização, o `LIBVA_DRIVER_NAME` ou o diretório de drivers mudam.

**Sintomas:** `no element "videoconvert"` (ou outro elemento) embora os plugins estejam instalados; um decodificador por hardware que o GStreamer usava antes não é mais usado.

**Correção:** apagar o registro; a próxima inicialização escreve um novo (essa inicialização demora um ou dois segundos a mais).

```bash
rm ~/.cache/veil/gstreamer-registry.bin          # the Nix package's (or under $XDG_CACHE_HOME)
rm ~/.cache/gstreamer-1.0/registry.*.bin         # GStreamer's shared one: every other build of veil
```

`GST_REGISTRY` (ou `GST_REGISTRY_1_0`), se definido, nomeia o arquivo em seu lugar. Para ver o que o registro deixou de fora, pergunte ao `gst-inspect-1.0` com o mesmo ambiente: `gst-inspect-1.0 -b` lista os plugins que ele pôs na lista negra. O pacote Nix mantém um registro próprio por esse motivo: o compartilhado é reescrito por todo programa GStreamer com seus próprios plugins (ele uma vez perdeu os plugins base na máquina do desenvolvedor, 2026-10-15).

Reproduzido: uma cópia de `libgstvideoconvertscale.so` danificada sem mudar seu tamanho nem seu timestamp fez o daemon falhar com `no element "videoconvert"` (o log tinha `Failed to load plugin ...: invalid ELF header`); reparada byte a byte, ele ainda falhava, e `gst-inspect-1.0 -b` a listava; com o registro apagado, o vídeo foi reproduzido.

## Plugins de outra versão do GStreamer

O `GST_PLUGIN_PATH` é pesquisado antes do conjunto de plugins que acompanha o veil (também antes do do pacote Nix). Os plugins ali que pertencem a outra versão do GStreamer são carregados no lugar dos do veil, e quebram os que sobram: com os plugins do GStreamer 1.26 em `GST_PLUGIN_PATH`, o `videoconvert` da 1.28 não pôde ser carregado (`Failed to load plugin '.../libgstvideoconvertscale.so': ... undefined symbol: gst_video_meta_transform_matrix_init`) e o daemon parou com `no element "videoconvert"`. Remover a variável não ajudou: o registro manteve a falha até ser apagado.

**Correção:** iniciar o veil sem `GST_PLUGIN_PATH` (sob o systemd: `systemctl --user edit veil`, depois `UnsetEnvironment=GST_PLUGIN_PATH` em `[Service]`) e apagar o registro como acima. `GST_DEBUG=GST_PLUGIN_LOADING:4` registra o arquivo de onde cada plugin é carregado.

## Sem decodificação por hardware

O log nomeia o decodificador de cada vídeo e diz se ele roda na GPU:

```text
INFO veil::engine::video: video decoder: nvh264dec (hardware)
INFO veil::engine::video: video decoder: avdec_h264 (software)
```

Um vídeo decodificado por software ainda é reproduzido; ele custa mais CPU. O que o GStreamer pode usar depende do sistema:

* **Intel e AMD:** os decodificadores `va` (`vah264dec`, `vah265dec`, `vavp9dec`, `vaav1dec`, ...) precisam de um driver VA-API (`intel-media-driver` ou o `radeonsi` do Mesa). `vainfo` (do `libva-utils`) mostra se algum funciona e quais codecs ele decodifica. No NixOS os drivers vêm do `hardware.graphics` (`hardware.graphics.enable = true`, mais `extraPackages` para o driver da Intel).
* **NVIDIA:** os decodificadores `nvcodec` (`nvh264dec`, `nvh265dec`, `nvvp9dec`, `nvav1dec`, ...) precisam das bibliotecas de CUDA e de decodificação de vídeo do driver proprietário; no NixOS elas vêm com o `hardware.nvidia`. O driver VA-API da NVIDIA só é usado com `GST_VA_ALL_DRIVERS=1`, e não é melhor aqui (problema conhecido 7).
* **O codec:** uma GPU decodifica apenas os codecs para os quais tem hardware (o AV1, por exemplo, só em placas recentes). Para os demais, o GStreamer escolhe um decodificador por software.
* **Nix em outra distribuição:** as bibliotecas compiladas pelo Nix procuram drivers em `/run/opengl-driver` e nos diretórios usuais do sistema, mas um driver compilado para a distribuição pode não carregar neles. Não testado.
* **Um registro escrito sem o driver** mantém os decodificadores da NVIDIA de fora ([Um registro do GStreamer desatualizado](#um-registro-do-gstreamer-desatualizado)).

A decodificação por hardware não torna um vídeo gratuito: na NVIDIA, todo quadro ainda é convertido para RGB na CPU (cerca de 18 % de um núcleo para 1080p a 30 fps). Em Intel e AMD, os quadros podem ir ao compositor sem isso (como dmabufs), o que não foi verificado ([`known-issues.md`](/apps/veil/docs/known-issues), entrada 7).

### Forçar a decodificação por software

Quando um decodificador por hardware se comporta mal (um bug do driver, um stream que ele mostra errado), retire-o da escolha do GStreamer dando-lhe o rank 0 (`NONE`): o GStreamer então escolhe o próximo decodificador para aquele formato. O nome é o que está no log:

```bash
GST_PLUGIN_FEATURE_RANK=nvh264dec:0 veil daemon
# several, comma-separated
GST_PLUGIN_FEATURE_RANK=nvh264dec:0,nvh265dec:0,nvvp9dec:0,nvav1dec:0 veil daemon
```

Medido: com `nvh264dec:0` (ou `nvh264dec:NONE`), o clipe H.264 foi decodificado pelo `avdec_h264 (software)`; com `nvvp9dec:0`, o clipe VP9 pelo `vp9dec (software)`. Sob o systemd, `systemctl --user edit veil` e acrescente `Environment=GST_PLUGIN_FEATURE_RANK=nvh264dec:0` em `[Service]`. A variável é lida quando o daemon inicia; ela não altera o registro. (Uma opção de decodificador própria do veil está planejada para depois da 1.0; veja [`roadmap.md`](/apps/veil/docs/roadmap).)

## Um vídeo abre em uma janela própria

**Sintoma:** uma janela normal (no sway: em mosaico, com barra de título, seu `app_id` o nome do programa) reproduz o vídeo, e o próprio papel de parede mostra apenas sua cor de fundo.

**Causa:** o `waylandsink` não recebeu a conexão Wayland do veil, então abriu a sua e criou uma janela ali. O veil entrega a conexão a ele em um contexto do GStreamer do tipo `GstWaylandDisplayHandleContextType` (`engine::video::display_context`), com o ponteiro do display no campo `handle`. O GStreamer 1.28.7 lê primeiro o campo `display` e depois `handle` "for backward compatibility" (`gst-libs/gst/wayland/gstwlcontext.c`); em um teste os dois nomes funcionaram, e um campo com outro nome produziu exatamente esta janela. O veil mantém `handle`, o campo que o GStreamer continua lendo por compatibilidade (qual campo as versões anteriores à 1.28 liam não foi verificado).

Isso não acontece com o veil como é lançado. Se isso aparecer, relate com a versão do GStreamer (a primeira linha do log) e um log com `GST_DEBUG=waylandsink:5`.

## Um nome de saída não é encontrado

Os nomes de saída são os do compositor (`DP-3`, `HDMI-A-1`, `eDP-1`, ...), e diferenciam maiúsculas de minúsculas. `veil outputs` lista as saídas que o compositor tem e em quais o daemon desenha; `hyprctl monitors` e `swaymsg -t get_outputs` dão os mesmos nomes.

| Onde | O que acontece |
|---|---|
| `veil daemon --output DP-9`, e nenhuma saída tem esse nome | O daemon não inicia (código de saída 1): `Output 'DP-9' was not found; available outputs: HEADLESS-1`. O mesmo para `--output headless-1` quando a saída é `HEADLESS-1`. |
| `--output DP-9 --output HDMI-A-1`, e só o `HDMI-A-1` existe | O daemon inicia e espera: `WARN veil::wayland: output DP-9 not connected; waiting for it (connected: HDMI-A-1)`. O `DP-9` recebe seu papel de parede quando for conectado. |
| `veil set --output DP-9` (também `clear`, `pause`, `resume`, `toggle`) | Recusado na hora, código de saída 1: `output 'DP-9' is not driven by this daemon; it draws on HEADLESS-1`. |
| `[output."DP-9"]` no arquivo de configuração | Usado quando uma saída com esse nome está conectada; `veil config check` não sabe quais saídas existem e não reclama. |

Uma saída que não tem nome (raro: um compositor que não envia `wl_output.name`) recebe o papel de parede padrão e não pode ser nomeada.

## Outras mensagens

| Mensagem | Significado |
|---|---|
| `GStreamer-Wayland-WARNING **: Could not bind to zwp_linux_dmabuf_v1` | O compositor não oferece dmabufs (um sway headless que renderiza na CPU, por exemplo). Os quadros passam pela memória compartilhada; nada mais muda. |
| `XDG_RUNTIME_DIR is not set to an absolute path; veil keeps its control socket there` | Não é uma sessão de desktop normal. Inicie o veil a partir da sessão (ou do gerenciador de usuário do systemd). |
| `veil daemon` termina com código de saída 4 | Já roda um daemon para este compositor. `veil quit` o encerra. |
| `Lost the connection to the Wayland compositor`, código de saída 5 | O compositor terminou ou falhou. O veil não reconecta; a próxima sessão o inicia de novo. |
| O daemon morre com SIGSEGV (código de saída 139) quando um monitor é alterado, conectado ou ligado enquanto um vídeo é reproduzido | Problema conhecido 1, um bug do `waylandsink` do GStreamer. Rode o veil sob sua unidade do systemd: ele volta em cerca de três segundos com seus papéis de parede. |
| Um erro no arquivo de configuração, código de saída 2 | A mensagem traz o arquivo, a linha e a coluna. `veil config check` diz o mesmo sem um daemon. |

## Guia rápido de depuração

| O que você quer | Comando |
|---|---|
| Há um daemon rodando? | `veil ping` (código de saída 3: nenhum) |
| O que cada saída mostra, seu estado, tamanho e escala, as contagens de quadros do vídeo | `veil status` (`--json` para scripts) |
| As saídas e seus nomes | `veil outputs` |
| O que o arquivo de configuração resulta em, e se seus papéis de parede podem ser exibidos | `veil config check` |
| O log sob o systemd | `journalctl --user -u veil -f` |
| Mais do veil: cada carregamento, troca, pausa e o que o compositor informa | `RUST_LOG=veil=debug veil daemon` |
| Tudo, inclusive as bibliotecas que o veil usa | `RUST_LOG=debug veil daemon` |
| O sink de vídeo: buffers, a janela, o retângulo de renderização | `GST_DEBUG=waylandsink:5 veil daemon` |
| De qual arquivo cada plugin do GStreamer é carregado, e quais falham | `GST_DEBUG=GST_PLUGIN_LOADING:4 veil daemon` |
| Se o `videocrop` recorta sem cópia (`in-place transform using crop meta`) | `GST_DEBUG=videocrop:5 veil daemon --mode fill` |
| Um log do GStreamer em um arquivo próprio | `GST_DEBUG_FILE=/tmp/gst.log` com qualquer uma das linhas `GST_DEBUG` |
| Toda requisição e evento do Wayland (as linhas `{GStreamer display queue}` são do sink de vídeo) | `WAYLAND_DEBUG=1 veil daemon` |
| Retirar um decodificador por hardware | `GST_PLUGIN_FEATURE_RANK=nvh264dec:0 veil daemon` |
| Um registro novo do GStreamer, para descartar um desatualizado | `GST_REGISTRY=/tmp/veil-registry.bin veil daemon` |
| Sob o systemd | `systemctl --user edit veil`, depois `Environment=RUST_LOG=veil=debug` (ou qualquer variável acima) em `[Service]`, e `systemctl --user restart veil` |

## Relatar um problema

Os relatos vão para o [Discussions, categoria "Reports"](https://github.com/ordreth/veil/discussions/categories/reports) (o veil não aceita issues nem pull requests; nada é prometido em troca). Um relato é mais útil com a primeira linha do log (as versões do veil e do GStreamer), `veil status`, o compositor e sua versão, a placa gráfica e seu driver, e a linha `video decoder:` para um vídeo. Diga o que você fez, o que esperava e o que aconteceu. Um problema de segurança não é relatado ali: veja [`SECURITY.md`](/apps/veil/docs/security).

## Matriz de suporte

O que foi executado, onde e por quem. Os rótulos: **Verificado** (executado e conferido pelo projeto), **Relatado como funcionando** (executado uma vez, pelas ferramentas do projeto ou por um usuário, ainda não conferido de novo), **Não testado**, **Não suportado**. Os detalhes de cada linha estão na [tabela de compatibilidade](#compatibilidade) abaixo, que é a fonte desta.

| | Status | O quê, quando |
|---|---|---|
| **Compositores** | | |
| sway (headless, 1.12) | Verificado | Toda verificação de `tests/headless.rs` e as medições; 2026-10-19. |
| Hyprland 0.56.2 | Relatado como funcionando | Imagens, vídeo H.264 em dois monitores e a pausa automática, 2026-10-20. Fluidez, bloqueio, DPMS, logout e recuperação de falhas estão **Não testados**. |
| river, niri, Wayfire, labwc, outros compositores com `wlr-layer-shell` | Não testado | Deveriam funcionar. Sem pausa automática (todo vídeo conta como visível), apenas a liberação de um vídeo que ninguém está vendo. |
| KDE Plasma (KWin) | Não testado | Tem `wlr-layer-shell`; ninguém tentou. |
| GNOME (Mutter) | Não suportado | Sem `wlr-layer-shell`. |
| **Gráficos** | | |
| NVIDIA (NVDEC) | Relatado como funcionando | RTX 3060 Ti, driver 595.91.07, sway e Hyprland. Todo quadro de vídeo é convertido na CPU ([problema conhecido 7](/apps/veil/docs/known-issues)). |
| Intel, AMD (VA-API) | Não testado | O caminho sem cópia não foi verificado em nenhum hardware. |
| Decodificação por software | Relatado como funcionando | Nos testes e retirando os decodificadores por hardware ([solução de problemas](#sem-decodificação-por-hardware)). |
| **Arquiteturas** | | |
| x86_64 | Relatado como funcionando | O tarball em um contêiner de seis distribuições, 2026-10-19. |
| aarch64 | Não testado | O workflow de lançamento o compila e roda os testes lá; ele nunca foi executado com um compositor. |
| **Distribuições** (o tarball pré-compilado, em um sway headless) | | |
| Debian 12 e 13, Ubuntu 22.04 e 24.04, Fedora 44, Arch Linux | Relatado como funcionando | 2026-10-19. GIFs animados exigem o GStreamer 1.26 ou mais recente, e o Ubuntu 24.04 perde quadros de vídeos de 1080p ([problema conhecido 8](/apps/veil/docs/known-issues)). |
| NixOS (o flake) | Verificado | `nix run` e `nix profile add`, com sway e Hyprland: imagens, vídeo e um daemon encerrado com `SIGKILL` substituído pelo seguinte. 2026-10-20 e 2026-11-04. O overlay com `systemd.packages` do [README](/apps/veil/docs/install) não foi executado nele. |

O que um usuário deve saber antes de usar o veil com vários monitores e um vídeo: um monitor que é alterado, conectado ou ligado enquanto um vídeo é reproduzido derruba o daemon ([problema conhecido 1](/apps/veil/docs/known-issues)); sob a unidade do systemd, ele volta em segundos.

## Compatibilidade

Em que o veil foi executado e o que funcionou. **Testadores: relatem uma linha** no [Discussions, categoria "Reports"](https://github.com/ordreth/veil/discussions/categories/reports): o compositor e sua versão, a placa gráfica e o driver, as versões do veil e do GStreamer (a primeira linha do log) e, para cada coluna, se funcionou, com o que foi tentado. "Ainda não" significa que ninguém relatou, não que falha.

| Compositor | GPU, driver | Imagens | Vídeo | Decodificador (do log) | Pausa automática | Observações | Verificado em |
|---|---|---|---|---|---|---|---|
| sway 1.12, headless, renderização na CPU (`pixman`) | NVIDIA RTX 3060 Ti, 595.91.07 | sim | sim: H.264, HEVC, VP8, VP9, AV1 | `nvh264dec`, `nvh265dec`, `nvav1dec`, `nvvp8dec`, `nvvp9dec` (hardware); os de software quando os de hardware são retirados | sim (IPC do sway) | Onde rodam todas as verificações de `tests/headless.rs` e as medições. Problema conhecido 1: uma mudança de monitor enquanto um vídeo é reproduzido mata o daemon. Os quadros são convertidos na CPU (problema conhecido 7). | veil 0.1.0, GStreamer 1.28.7, 2026-10-19 |
| sway 1.12, headless, renderização na GPU (`gles2`) | NVIDIA RTX 3060 Ti, 595.91.07 | ainda não | ainda não | | ainda não | O pipeline de sink do veil foi executado aqui com `gst-launch-1.0` (o veil em si não foi executado): o sway oferece dmabufs e só aceita os NV12 com os modificadores tiled da NVIDIA. | |
| Hyprland 0.56.2 | NVIDIA RTX 3060 Ti (a máquina da primeira linha) | sim (como papel de parede diário, pela unidade do systemd) | sim: H.264 1080p60 em dois monitores 1080p ao mesmo tempo | `nvh264dec` (hardware) | sim: uma janela em tela cheia pausou sua saída ("cannot be seen; pausing it"), e fechá-la voltou a reproduzir; a outra saída continuou reproduzindo | Cerca de 90 a 99 % de um núcleo para dois streams 1080p60 (problema conhecido 7), cerca de 41 % com uma saída pausada. Fluidez sem atraso (problema conhecido 6), bloqueio, DPMS, logout, recuperação de falhas e uma mudança de monitor não foram verificados no Hyprland. O estado que o veil lê do seu IPC coincide com `hyprctl monitors`. | veil 0.1.0 (uma compilação de lançamento da árvore), GStreamer 1.28.7, 2026-10-20 |
| sway, em hardware | qualquer | ainda não | ainda não | | | | |
| Hyprland | Intel ou AMD (VA-API) | ainda não | ainda não | | | Mostraria se o caminho sem cópia funciona (problema conhecido 7, opção A). | |
| river, niri, Wayfire, labwc, KDE Plasma (wlr-layer-shell) | qualquer | ainda não | ainda não | | não (pausa apenas atrás de uma tela bloqueada ou de um monitor desligado) | Deveria funcionar: o veil precisa de `wlr-layer-shell`; `wp_viewporter` e `wp_fractional_scale_v1` são usados quando estão presentes. | |
| GNOME (Mutter) | qualquer | não | não | | | Não tem `wlr-layer-shell`, que o veil exige. | |
