---
title: "Changelog"
description: "O que muda para os usuários, lançamento a lançamento."
group: "Project"
order: 11
source: "CHANGELOG.md"
source_hash: "659a87d42b909df606804da78d6deeeb879986bd78deec107c74f546baee1a96"
---

Toda mudança que um usuário do veil pode perceber é registrada aqui. O formato segue o [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), e o veil usa o [versionamento semântico](https://semver.org/spec/v2.0.0.html) a partir da 1.0. O que conta como interface do veil para o número da versão: a linha de comando (comandos, flags, valores, o que o `--json` imprime), os códigos de saída, o protocolo do socket de controle ([`docs/ipc.md`](/apps/veil/docs/ipc)), as chaves do arquivo de configuração ([`docs/configuration.md`](/apps/veil/docs/configuration)) e o arquivo de estado.

O veil 1.0.0-beta.1 é o primeiro lançamento, um beta. Tudo o que está abaixo faz parte dele. As limitações conhecidas e deixadas em aberto de propósito estão em [`docs/known-issues.md`](/apps/veil/docs/known-issues); as mais importantes são listadas ao fim da seção.

## [Não lançado]

### Adicionado

- Uma política de privacidade, [`PRIVACY.md`](/apps/veil/privacy): o que o veil lê e escreve no seu computador, e que nada sai dele. Os tarballs de lançamento agora a trazem, e o `SECURITY.md` também.
- O README diz como atualizar o veil para cada forma de instalá-lo.

### Corrigido

- Um monitor alterado (escala, modo, posição), conectado, desconectado, ligado ou despertado do DPMS enquanto um vídeo é reproduzido não derruba mais o daemon (problema conhecido 1). A falha estava no `waylandsink` do GStreamer, que o veil não usa mais.
- Um vídeo que foi interrompido porque ninguém o via (uma sessão bloqueada, um monitor desligado) não mostra mais só a cor de fundo quando volta a ser visto: seu último quadro fica na tela até que o vídeo, iniciado de novo, tenha seu primeiro quadro.
- No Hyprland 0.57, o veil volta a ler os monitores (`cannot read the answer to j/monitors: missing field 'id'`), então um vídeo também pausa lá sob uma janela em tela cheia ou que o cubra. O Hyprland 0.57 omite o id do workspace especial de um monitor quando nenhum é exibido.
- Vídeos decodificados por um decodificador VA-API (Intel, AMD) e convertidos na CPU não parecem mais desbotados (problema conhecido 9): o decodificador não informa a faixa de cores do vídeo, e o conversor do GStreamer a tomava pela completa. O veil agora toma uma faixa desconhecida pela limitada, a do vídeo, como já fazia com quadros em dmabufs. No Hyprland até a 0.57, isso vale para todo vídeo em Intel e AMD.

### Alterado

- Lançamentos e `main` são separados. O branch `stable` é o lançamento mais recente, e as instruções do README para o Nix e para compilar a partir do código-fonte agora o usam (`github:ordreth/veil/stable`); o `main` é o trabalho rumo ao próximo lançamento, e sua versão termina em `-dev` (`1.0.0-beta.2-dev`). Uma entrada de flake em `github:ordreth/veil` continua seguindo o `main`.
- Os vídeos são exibidos pelo sink de vídeo próprio do veil, em uma subsuperfície própria do veil, em vez do `waylandsink` do GStreamer. Cada quadro é convertido e recortado em uma única passada direto na memória que o compositor lê (o que `fill` e `center` cortam não é convertido), e o compositor o escala, como antes. O laço de eventos do daemon acorda cerca de metade das vezes enquanto um vídeo é reproduzido (34 em vez de 85 vezes por segundo a 30 fps); a CPU que um vídeo custa é a mesma. Em um compositor que retém o quadro que exibe (um sway headless), um vídeo 4K ocupa cerca de 100 MB a mais de memória do que antes: três buffers de 33 MB.
- Os vídeos precisam do `gst-plugins-bad` só para os decodificadores por hardware; `waylandsink`, `videoconvert` e `videocrop` não são mais usados.
- Um vídeo cujo monitor some por um momento (desconectado, desativado, um link que cai) continua de onde estava quando o monitor volta, em vez de recomeçar do início: seu pipeline é mantido, pausado, pelo tempo que `--release-unseen-after` mantém um vídeo não visto (2 s por padrão; com `0`, até o monitor voltar). Um monitor que fica fora por mais tempo recebe o vídeo do início, como antes.
- Um vídeo decodificado por um decodificador VA-API (Intel, AMD) vai para o compositor como está, nos dmabufs do decodificador, quando o compositor aceita o formato deles (`zwp_linux_dmabuf_v1`) e pode ser informado de como as cores do vídeo estão codificadas (`wp_color_representation_v1`; sem ele, só para vídeos codificados como BT.601): nada é convertido nem copiado na CPU, e o compositor recorta e escala os quadros. Em um notebook Intel (Raptor Lake) com um sway headless renderizando na GPU, H.264 em 1080p30 custa cerca de 3 % de um núcleo em vez de 45 %, e 4K a 60 fps é reproduzido por inteiro em vez de perder um terço dos quadros. Outros decodificadores (os da NVIDIA) e compositores que não aceitam os quadros são convertidos na CPU, como antes; o log diz por qual caminho um vídeo vai, e diz no início quando o compositor não aceita nenhum dmabuf YUV (Hyprland até a 0.57: todos os seus vídeos são convertidos na CPU).

## [1.0.0-beta.1] - 2026-10-20

### Adicionado

- O veil é de código-fonte disponível sob a PolyForm Strict License 1.0.0 com permissões adicionais (`ADDITIONAL-PERMISSIONS.md`), de uso gratuito para fins não comerciais; não é código aberto. Os tarballs de lançamento e o pacote do Nix incluem `LICENSE`, `ADDITIONAL-PERMISSIONS.md` e `THIRD-PARTY-NOTICES`, as licenças dos crates Rust que o veil usa.
- `veil daemon`: um papel de parede em cada saída de um compositor Wayland com `wlr-layer-shell` (Hyprland, sway e outros compositores wlroots), também nas saídas conectadas depois, ou apenas nas saídas indicadas com `--output`. Uma saída que é desconectada recebe de volta o seu papel de parede quando retorna.
- Imagens nos formatos do crate `image` (PNG, JPEG, WebP, BMP, TIFF, GIF, ...), decodificadas uma única vez e compostas na resolução da saída; imagens com mais de 16384 pixels por lado, ou que precisariam de mais de 512 MiB, são recusadas antes de serem decodificadas.
- Papéis de parede em vídeo pelo GStreamer (`waylandsink`), sem áudio, em loop sem reconstruir o pipeline, decodificados por hardware quando o GStreamer tem um decodificador para a placa. O log informa o decodificador.
- Imagens e vídeos são distinguidos pelo conteúdo do arquivo. GIFs animados são reproduzidos como vídeos. Arquivos que não podem ser um papel de parede (um diretório, um arquivo vazio, HEIF, JPEG XL, algo desconhecido) são recusados com o motivo.
- Os modos de ajuste `fill`, `fit`, `stretch`, `center` e `tile` (`--mode`) e uma cor de fundo para as barras (`--bg-color`), para imagens e vídeos.
- Escalas HiDPI e fracionárias (`wp_viewporter`, `wp_fractional_scale_v1`): as imagens são desenhadas na resolução física da saída.
- Um novo papel de parede substitui o antigo apenas quando já tem algo para mostrar (para um vídeo, o seu primeiro quadro); um que falha deixa o antigo na tela.
- Um vídeo que falha durante a reprodução é reiniciado (no máximo quatro vezes seguidas) e depois abandonado, com uma única notificação de desktop. As notificações são limitadas em frequência.
- Pausa automática de um vídeo atrás de uma janela em tela cheia ou, com `--pause-on covered`, atrás de janelas que cobrem a saída inteira, lida do IPC do Hyprland ou do sway, por saída.
- Um vídeo que ninguém vê (uma tela bloqueada, um monitor desligado) é parado após dois segundos e iniciado de novo quando a saída volta a ser desenhada, em qualquer compositor (`--release-unseen-after`).
- Comandos para um daemon em execução: `veil set`, `clear`, `pause`, `resume` e `toggle`, para todas as saídas ou para uma (`--output`), `status` e `outputs` (ambos com `--json`), `quit` (alias `stop`) e `ping`, com códigos de saída estáveis.
- Um socket de controle em `XDG_RUNTIME_DIR` para outros programas (barras de status, scripts), um objeto JSON por linha, versionado e apenas para processos do mesmo usuário. Referência: [`docs/ipc.md`](/apps/veil/docs/ipc).
- O papel de parede de cada saída é salvo (`$XDG_STATE_HOME/veil/state.json`) e exibido de novo após uma reinicialização; `veil set --no-persist` dispensa isso.
- Um arquivo de configuração, `~/.config/veil/config.toml`: as opções do daemon e o papel de parede de cada saída, verificado com `veil config check`. Referência: [`docs/configuration.md`](/apps/veil/docs/configuration), exemplo: `contrib/config.example.toml`.
- Uma unidade de usuário do systemd (`contrib/systemd/veil.service`): ativa quando todas as saídas exibem o seu papel de parede, reiniciada após uma falha grave com os papéis de parede que tinha, com um watchdog no loop de eventos.
- Um daemon por compositor; um segundo termina com código de saída 4.
- Autocompletar de shell para bash, zsh, fish e nushell (`veil completions`) e uma página de manual por comando.
- Um flake do Nix: o pacote (envolvido com os seus plugins do GStreamer e um registro do GStreamer próprio), `nix run`, um overlay e dev shells.
- Binários pré-compilados para Linux x86_64 e aarch64 em todo lançamento: um tarball com o binário, a unidade do systemd, as páginas de manual, o autocompletar de shell, os documentos e o `install.sh` (em `/usr/local`, ou `--prefix ~/.local`; `--uninstall`), com `SHA256SUMS` e uma atestação de proveniência de compilação por tarball. Eles usam o GStreamer do sistema e exigem a glibc 2.34 ou mais nova; rodam no Debian 12 e 13, no Ubuntu 22.04 e 24.04, no Fedora 44 e no Arch Linux.
- Documentação: um README com um guia rápido, os recursos e a sua situação, os números medidos e a instalação a partir do código-fonte no Arch, Debian, Ubuntu e Fedora; [solução de problemas e uma tabela de compatibilidade](/apps/veil/docs/troubleshooting), [o que foi medido](/apps/veil/docs/measurements), [problemas conhecidos](/apps/veil/docs/known-issues).

### Obsoleto

- `veil --daemon ...`, a linha de comando do primeiro protótipo. Ela ainda inicia o daemon, com um aviso; use `veil daemon ...`.

### Problemas conhecidos

- Um monitor que é alterado, conectado ou ligado enquanto um vídeo é reproduzido derruba o daemon, dentro do `waylandsink` do GStreamer (problema conhecido 1). Sob a unidade do systemd, ele volta em segundos com os seus papéis de parede.
- Na NVIDIA, todo quadro de vídeo é convertido para RGB na CPU: cerca de 18 % de um núcleo para 1080p a 30 fps (problema conhecido 7).
- Sem `MALLOC_ARENA_MAX=1`, que o pacote do Nix e a unidade do systemd definem, a memória cresce a cada troca de vídeo (problema conhecido 3).
- Com o binário pré-compilado em um GStreamer anterior à 1.26 (Debian 12, Ubuntu 22.04 e 24.04), GIFs animados ficam parados ou não iniciam, por causa do demuxer de GIF do GStreamer (no Debian 12 ele também mantém um núcleo de CPU ocupado); convertidos para vídeo, eles são reproduzidos. No Ubuntu 24.04, vídeos 1080p mostraram apenas parte dos seus quadros em um sway headless (problema conhecido 8).
- Tudo até agora foi verificado em um sway headless com uma placa NVIDIA; as verificações em uma sessão real do Hyprland e em placas Intel e AMD ainda estão por fazer.

[Não lançado]: https://github.com/ordreth/veil/compare/v1.0.0-beta.1...HEAD
[1.0.0-beta.1]: https://github.com/ordreth/veil/releases/tag/v1.0.0-beta.1
