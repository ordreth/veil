---
title: "Política de segurança"
description: "Como relatar um problema de segurança e o que o veil faz a respeito da sua superfície de ataque."
group: "Legal"
order: 16
source: "SECURITY.md"
source_hash: "da13ce0248791a485db04678487e972d72c61ff19995b6f9ec527891dddbf8ca"
---

## Relatar uma vulnerabilidade

Por favor, relate uma vulnerabilidade de forma privada, pelo GitHub: na aba **Security** do repositório, **Report a vulnerability** ([link direto](https://github.com/ordreth/veil/security/advisories/new)). Não a publique em uma discussão pública. Se você não puder usar o GitHub, escreva para contact@ordreth.com com "veil security" no assunto; não coloque os detalhes de um problema ainda não corrigido em um lugar público.

Um relato útil informa:

- a versão (a primeira linha do log do daemon: `veil 1.0.0-beta.1 (GStreamer 1.28.7)`) ou o commit,
- o que um atacante precisa: um processo de outro usuário, um arquivo que alguém exibe como papel de parede, um compositor, ...,
- o que ele pode fazer com isso e como reproduzir (um arquivo que o provoque, se houver).

Você será informado quando o relato tiver sido lido e do que está planejado. Uma correção é publicada junto com um aviso de segurança do GitHub, que dá o seu crédito se você quiser. Não há prazo fixo.

## Versões suportadas

O veil está em beta. As correções vão para o `main` e para um novo lançamento; somente o lançamento mais recente (o branch `stable`) as recebe. Uma compilação do `main` tem uma versão terminada em `-dev`: informe também o commit dela.

## Verificar um lançamento

Os tarballs de lançamento são compilados somente pelo workflow de lançamento (`.github/workflows/release.yml`), nas máquinas do GitHub, a partir do commit da tag, em um contêiner Debian 12 fixado pelo seu digest (`scripts/release/build.sh`). Cada lançamento lista a soma SHA-256 de cada tarball em `SHA256SUMS` e tem uma atestação assinada de proveniência de compilação por tarball, que nomeia o repositório, o workflow e o commit: `gh attestation verify veil-<version>-<target>.tar.gz --repo ordreth/veil`. A compilação ainda não é reproduzível bit a bit (duas compilações do mesmo commit diferem em algumas centenas de bytes de código de máquina), então um tarball não pode ser verificado compilando-o de novo; a atestação é a verificação. O veil não é publicado no crates.io.

## No que o veil confia

O veil é o daemon de papel de parede do próprio usuário. Ele roda com os direitos do usuário e não precisa de outros, não abre nenhuma conexão de rede e não escuta em nada além do seu socket de controle Unix. No que ele confia e no que não confia:

**O socket de controle.** Ele é `$XDG_RUNTIME_DIR/veil-$WAYLAND_DISPLAY.sock`, com modo `0600`, em um diretório que pertence ao próprio usuário; não há fallback para `/tmp` nem para outro diretório compartilhado. O daemon também verifica o ID de usuário de cada cliente (`SO_PEERCRED`) e responde `forbidden` a um processo de outro usuário sem ler a sua requisição; o `veil` não envia requisições a um socket em que outro usuário escuta. Um cliente do mesmo usuário é considerado confiável para tudo o que os comandos fazem: ele pode fazer o daemon exibir qualquer arquivo que o usuário possa ler, pausá-lo, pará-lo e ler os caminhos dos seus papéis de parede e os nomes das saídas (`veil status`). Nenhum conteúdo de arquivo volta para ele. Assim, o socket não dá nada além dos direitos do próprio usuário, **mas um programa em sandbox que receba acesso ao socket pode fazer o veil abrir arquivos fora do seu sandbox** e exibi-los na tela.

Um cliente não pode bloquear o daemon nem fazê-lo terminar: as requisições têm um limite de tempo (2 s) e um limite de tamanho (64 KiB), no máximo 16 conexões são atendidas ao mesmo tempo, o daemon nunca espera por um cliente e uma requisição malformada é respondida com um erro. Os limites estão em [`docs/ipc.md`](/apps/veil/docs/ipc#limites).

**Arquivos de mídia.** Um papel de parede é uma entrada não confiável, e o código que o lê é a maior parte da superfície de ataque do veil.

- As imagens são decodificadas pelo crate `image` (Rust). Antes de qualquer decodificação, uma imagem com mais de 16384 pixels por lado, ou que precisaria de mais de 512 MiB, é recusada.
- Os vídeos são lidos e decodificados pelo GStreamer e seus plugins (C, e o FFmpeg por meio do `gst-libav`), dentro do processo do daemon e com os direitos do usuário. Um arquivo feito para explorar um bug em um demuxer ou em um decodificador pode, portanto, executar código como o usuário, como pode em qualquer reprodutor de mídia. Os bugs dessas bibliotecas pertencem ao [GStreamer](https://gstreamer.freedesktop.org/security/) ou ao [FFmpeg](https://ffmpeg.org/security.html); avise-nos também se o veil deve fazer algo a respeito deles.
- O veil decide entre imagem e vídeo pelos primeiros 512 bytes do arquivo e recusa diretórios, dispositivos e pipes antes que um decodificador os veja.

**O compositor.** O compositor Wayland é confiável: ele tem a superfície do veil de qualquer modo. Os sockets de IPC do Hyprland e do sway (em `$XDG_RUNTIME_DIR/hypr/` e em `$SWAYSOCK`) são lidos sem bloquear e com limites de tempo, e uma resposta que não pode ser interpretada nunca encerra o daemon.

**O ambiente.** Quem define o ambiente do daemon pode executar código como o usuário (o GStreamer carrega plugins de `GST_PLUGIN_PATH`, por exemplo), então esse ambiente é confiável, como para qualquer programa. O mesmo vale para o arquivo de configuração, que é lido uma única vez na inicialização.

**O que o veil escreve.** O arquivo de estado (`$XDG_STATE_HOME/veil/state.json`) é escrito com modo `0600` em diretórios com modo `0700`, em um arquivo temporário que é renomeado sobre ele. O socket de controle e o seu arquivo de lock têm modo `0600`. As notificações de desktop passam pelo barramento de sessão e podem conter os caminhos dos papéis de parede.

**O código.** O `unsafe` é permitido em um único módulo, `engine::video` (`src/engine/video.rs` e `src/engine/video/`), e o compilador o recusa em todos os outros lugares. Esse módulo cria uma subclasse de um elemento do GStreamer (o sink de vídeo próprio do veil), e as macros do `glib` que fazem isso se expandem em código `unsafe`; o `unsafe` dele mesmo é um único bloco, em `src/engine/video/sink.rs`, que pega emprestado do GStreamer o descritor de arquivo do dmabuf de um quadro, com o motivo ao lado (o clippy recusa um bloco `unsafe` sem ele). Um panic encerra o daemon (a compilação de lançamento aborta) em vez de deixá-lo em um estado quebrado.

## O que está no escopo

Relate de forma privada qualquer coisa que permita a alguém fazer o que o usuário não permitiu: um processo de outro usuário fazendo o daemon fazer qualquer coisa, um arquivo que o veil escreve sendo legível por outro usuário ou escrito em outro lugar, uma requisição ou uma resposta do compositor que corrompe a memória ou executa código, uma imagem que contorna os limites de tamanho.

Não é uma vulnerabilidade: o que os processos do próprio usuário podem fazer pelo socket (acima) e, na 1.0.0-beta.1, a falha grave do problema conhecido 1 (uma mudança de monitor enquanto um vídeo é reproduzido, [`docs/known-issues.md`](/apps/veil/docs/known-issues); corrigida no `main`). Uma falha grave ou um travamento causado por uma requisição malformada ou por uma resposta do compositor que não faz nada do que foi dito acima é um bug: relate-o no [Discussions, categoria "Reports"](https://github.com/ordreth/veil/discussions/categories/reports).
