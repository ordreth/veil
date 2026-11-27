---
title: "Uso"
description: "Iniciar o daemon, definir papéis de parede, os comandos de controle, o serviço de usuário do systemd e scripts."
group: "Start"
order: 2
source: "README.md"
source_hash: "cdd7532edb15796b4d5a624320e3bb62322ea41ff9e71b334e8311d0609a62be"
---

## Iniciar o daemon

```bash
veil daemon                                           # every monitor: its wallpaper from the configuration file, else the saved one
veil daemon --input ~/Videos/waves.mp4                # this wallpaper on every monitor
veil daemon --output DP-3 --output HDMI-A-1           # only these monitors
veil daemon --mode fit --bg-color '#202020'           # letterbox on dark grey (quote the #)
```

Sem `--input`, cada monitor exibe o papel de parede do [arquivo de configuração](#arquivo-de-configuração), ou o que [exibia antes](#papéis-de-parede-salvos), ou uma `--bg-color` sólida (preto por padrão) até o `veil set`. Os nomes dos monitores são os do compositor (`veil outputs`, `hyprctl monitors`, `swaymsg -t get_outputs`). Um monitor mantém o papel de parede pelo nome: desconectado e conectado de novo, ele exibe o que exibia. Um monitor conectado pela primeira vez recebe o papel de parede padrão: o do último `veil set` para todos os monitores ou, na falta dele, `--input` ou o `[output."*"]` do arquivo de configuração.

`--mode` define como um papel de parede é ajustado ao seu monitor; o `--mode` e o `--bg-color` do daemon também são os padrões de `veil set`:

| Modo | Resultado |
|---|---|
| `fill` (padrão) | Cobre o monitor inteiro, mantendo a proporção. O excesso é cortado por igual dos dois lados. |
| `fit` | A imagem inteira fica visível, mantendo a proporção. Barras de `--bg-color` preenchem o resto. |
| `stretch` | Ajustado exatamente ao tamanho do monitor; a proporção não é mantida. |
| `center` | Tamanho nativo (um pixel por pixel do dispositivo), centralizado. Cortado se for maior que o monitor, cercado por `--bg-color` se for menor. |
| `tile` | Tamanho nativo, repetido a partir do canto superior esquerdo. Vídeos voltam para `center`. |

`veil daemon --help` (ou `man veil-daemon`) lista todas as opções.

## Imagens e vídeos

Se um arquivo é uma imagem ou um vídeo, isso é lido do seu conteúdo, não do seu nome:

| O arquivo é | É exibido como |
|---|---|
| Uma imagem (PNG, JPEG, WebP, BMP, TIFF e os demais formatos do crate `image`) | Uma imagem, seja qual for a extensão. |
| Um GIF com um quadro | Uma imagem. |
| Um GIF com vários quadros | Um vídeo (em loop). O GStreamer precisa do `gst-libav` para isso. |
| Um WebP ou PNG animado | Uma imagem: o seu primeiro quadro. Para WebP, um aviso é registrado no log. |
| Um contêiner de vídeo (Matroska, WebM, MP4, QuickTime, 3GP, AVI, Ogg, FLV, ASF, MPEG program ou transport stream) | Um vídeo, seja qual for a extensão. Se pode ser reproduzido depende dos plugins do GStreamer instalados. |
| Outra coisa, com extensão de vídeo (`mp4`, `m4v`, `mov`, `mkv`, `webm`, `avi`, `ogv`, `mpg`, `mpeg`, `ts`, `m2ts`, `mts`, `wmv`, `asf`, `flv`, `3gp`, `vob`) | Entregue ao GStreamer, que conhece mais contêineres; a resposta dele é o resultado. |
| Outra coisa, com extensão de imagem (por exemplo `tga`, um formato sem assinatura) | Entregue ao decodificador de imagens. |
| Uma imagem HEIC/HEIF ou JPEG XL | Recusada: `'photo.heic' is a picture in the HEIF format, which veil cannot decode`. (Uma imagem AVIF é recusada pelo decodificador de imagens: `The image format Avif is not supported`.) |
| Qualquer outra coisa | Recusada: `'notes.txt' is neither an image nor a video that veil recognises, by its content or by its extension`. |

Um caminho que não existe, um diretório, um arquivo vazio e um arquivo que não pode ser lido são recusados exatamente com esse motivo. Um vídeo com um único quadro é exibido como uma imagem estática, sem loop.

## Trocar o papel de parede

```bash
veil set --input ~/Videos/waves.mp4                    # every monitor
veil set --input ~/Pictures/photo.jpg --output DP-3    # one monitor; the others keep theirs
veil set --input ~/Pictures/pattern.png --mode tile    # another mode than the daemon's
veil set --input ~/Pictures/preview.png --no-persist   # not saved: a restart shows what was saved before
```

O daemon carrega o arquivo em segundo plano, e `veil set` retorna quando o papel de parede está na tela (para um vídeo, o primeiro quadro) em todos os monitores a que se destina. Se o arquivo não existe ou não pode ser exibido, `veil set` imprime o motivo e termina com código de saída 1, e o papel de parede antigo permanece. Um caminho relativo funciona: `veil set` o torna absoluto. Se vários `veil set` chegarem mais rápido do que os arquivos carregam, o último vence em cada monitor; um que seja substituído antes de ser exibido informa isso e termina com código de saída 1. Sem `--output`, o papel de parede também passa a ser o padrão para monitores conectados depois (no caso de um vídeo, depois que ele foi exibido).

## Controlar um daemon em execução

```bash
veil clear                     # only the background colour (saved like `veil set`)
veil pause                     # pause videos until `veil resume`, whatever is on screen
veil resume --output DP-3
veil toggle                    # prints `paused` or `playing`
veil status                    # what each monitor shows (--json for scripts)
veil outputs                   # the compositor's monitors, and which ones veil draws on
veil quit                      # stop the daemon; returns once it has ended (alias: veil stop)
veil ping                      # prints `pong` if a daemon runs, else exits with status 3
```

`veil status` imprime uma linha por monitor:

```text
veil 1.0.0-beta.1 (pid 512958), running for 7s; videos pause on: fullscreen
default: ~/Videos/waves.mp4 (fill, #000000)
DP-3: paused (by veil pause) ~/Videos/waves.mp4 (fill, #000000); 1920x1080 at scale 1; video 1280x720, 30 fps, 0 frames shown, 0 dropped
HDMI-A-1: showing ~/Pictures/wallpaper.png (fit, #000000); 2048x1152 at scale 1.25
```

Todo comando aceita `--help` e tem uma página de manual (`man veil-set`, ...). O que `veil pause` mantém dura até o `veil resume` ou até o fim do daemon; não é salvo.

## Pausa automática

No Hyprland e no sway, o veil acompanha o IPC do compositor e pausa um vídeo que ninguém consegue ver: ele mantém o quadro na tela e não decodifica nada, e continua do mesmo quadro quando volta a ser visível. `--pause-on` (ou `pause_on` no arquivo de configuração) define quando:

| `--pause-on` | Um vídeo é pausado enquanto o seu monitor mostra |
|---|---|
| `never` | Nada o pausa. |
| `fullscreen` (padrão) | Uma janela em tela cheia. Uma janela maximizada não conta. |
| `covered` | Uma janela em tela cheia, ou janelas que cobrem o monitor inteiro (espaços ou uma barra o deixam descoberto). Uma janela translúcida conta como cobertura. |

Um vídeo é pausado depois de ficar oculto por um segundo, então alternar entre áreas de trabalho não o interrompe a cada vez, e ele volta a ser reproduzido de imediato. Em outros compositores, nada é pausado dessa forma.

Em qualquer compositor, um vídeo cujos quadros o compositor não aceita por `--release-unseen-after` segundos (padrão 2) é interrompido e seu pipeline é liberado: a tela está bloqueada, o monitor está desligado ou (no sway) uma janela o cobre. Ele recomeça do início quando o monitor volta a exibi-lo. `0` desativa isso.

## Arquivo de configuração

`~/.config/veil/config.toml` (ou `$XDG_CONFIG_HOME/veil/config.toml`, ou `veil daemon --config FILE`) guarda as opções do daemon e o papel de parede de cada monitor:

```toml
[defaults]
mode = "fill"           # --mode
bg_color = "#000000"    # --bg-color
pause_on = "covered"    # --pause-on

[output."DP-3"]
input = "~/Videos/waves.mp4"
mode = "fit"

[output."*"]            # every monitor without a section of its own
input = "~/Pictures/wallpaper.png"
```

Uma opção na linha de comando vale mais que o arquivo, e o arquivo vale mais que o padrão embutido. Um monitor com papel de parede no arquivo o exibe a cada início; `veil set` o muda até o daemon parar. O arquivo só é lido quando o daemon inicia. `veil config check` o lê como o daemon o leria, verifica seus papéis de parede e imprime com o que o daemon iniciaria; uma chave ou um valor incorreto é informado com a linha e a coluna. Todas as chaves estão em [`docs/configuration.md`](/apps/veil/docs/configuration), e [`contrib/config.example.toml`](https://github.com/ordreth/veil/blob/main/contrib/config.example.toml) é um arquivo comentado.

## Papéis de parede salvos

Todo `veil set` e `veil clear` que é exibido é salvo, por monitor, em `$XDG_STATE_HOME/veil/state.json` (`~/.local/state/veil/state.json`), com seu modo e sua cor. Um daemon iniciado sem `--input` exibe de novo o papel de parede salvo de cada monitor, a menos que o arquivo de configuração dê um a esse monitor; iniciado com `--input`, ele o exibe em todos os monitores e o salva. `--no-persist` exibe um papel de parede sem salvá-lo. Um arquivo salvo que sumiu é registrado no log, e seu monitor exibe a cor de fundo. O arquivo é compartilhado por todos os daemons de um usuário: um daemon iniciado para um teste em um compositor aninhado também salva ali, a menos que se dê a ele outro `XDG_STATE_HOME`.

## Executar como serviço de usuário do systemd

A unidade ([`contrib/systemd/veil.service`](https://github.com/ordreth/veil/blob/main/contrib/systemd/veil.service)) executa `veil daemon` sem opções, então exibe os papéis de parede do arquivo de configuração e os salvos. Instalada como acima (ou pelo pacote do Nix):

```bash
systemctl --user daemon-reload
systemctl --user enable --now veil
```

- `systemctl --user start veil` retorna quando todos os monitores exibem seu papel de parede, e `systemctl --user status veil` informa como foi a inicialização.
- Após uma falha (como a das mudanças de monitor, [problema conhecido 1](/apps/veil/docs/known-issues)), ele é reiniciado depois de 2 segundos, com os papéis de parede que tinha. Ele não é reiniciado após um arquivo de configuração incorreto (corrija-o e depois `systemctl --user restart veil`), um segundo daemon ou o fim da sessão.
- Um watchdog reinicia um daemon cujo loop de eventos travou, e `MALLOC_ARENA_MAX=1` mantém a memória estável ao longo das trocas de vídeo.
- O log fica no journal: `journalctl --user -u veil`.

A unidade inicia com o `graphical-session.target` e precisa de `WAYLAND_DISPLAY` no ambiente do gerenciador de usuário (e de `HYPRLAND_INSTANCE_SIGNATURE` ou `SWAYSOCK` para a pausa automática). O Hyprland as importa por conta própria. Uma sessão sem `graphical-session.target` a inicia a partir da própria configuração, em vez de habilitá-la:

```bash
# Hyprland (hyprland.conf)
exec-once = systemctl --user start veil
# sway (config)
exec systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK && systemctl --user start veil
```

As opções do daemon vão no arquivo de configuração, ou em `ExecStart` com `systemctl --user edit veil`. Use `systemctl --user stop veil`, em vez de `veil quit`, para parar o serviço.

## Autocompletar do shell e páginas de manual

O pacote do Nix instala ambos. Fora dele, o binário imprime o script de autocompletar de um shell e grava as páginas de manual:

```bash
veil completions bash > ~/.local/share/bash-completion/completions/veil
veil completions zsh > ~/.zfunc/_veil            # with fpath+=(~/.zfunc) before compinit in ~/.zshrc
veil completions fish > ~/.config/fish/completions/veil.fish
veil man-pages ~/.local/share/man/man1           # veil.1, veil-set.1, ...
```

No nushell: `mkdir ($nu.user-autoload-dirs | first); veil completions nushell | save -f ($nu.user-autoload-dirs | first | path join veil.nu)`. Crie os diretórios antes, se não existirem. Os nomes dos monitores não são completados.

## Scripts e barras de status

Os comandos `veil` falam com o daemon por um socket Unix, `$XDG_RUNTIME_DIR/veil-$WAYLAND_DISPLAY.sock`, que outros programas também podem usar: uma requisição JSON e uma resposta JSON por conexão, apenas para processos do seu próprio usuário. `veil status --json` e `veil outputs --json` imprimem as mesmas respostas. Campos, comandos e códigos de erro só são acrescentados, nunca renomeados nem removidos, e os códigos de saída do `veil` são estáveis (por exemplo, 3 quando nenhum daemon está em execução). A referência do protocolo, dos campos JSON e dos códigos de saída é [`docs/ipc.md`](/apps/veil/docs/ipc); os códigos de saída também estão em `man veil`.
