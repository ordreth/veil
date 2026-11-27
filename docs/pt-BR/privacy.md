---
title: "Política de privacidade"
description: "O que o veil lê e escreve no seu computador, e que nada sai dele."
group: "Legal"
order: 14
source: "PRIVACY.md"
source_hash: "cd55f7bf6693bfd9d26b9a61e03a46dbd872109a6bcb320a12105bd89a3fd198"
---

**veil** · vale para o veil 1.0.0-beta.1 e posteriores · última atualização em 2026-11-27

> Qualquer coisa aqui que o código do veil não confirme é um bug: avise-nos em contact@ordreth.com.

## Em resumo

- O veil é um daemon de papel de parede que roda no seu computador. Ele não coleta nada e não envia nada: nenhuma
  conta, nenhuma telemetria, nenhuma análise de uso, nenhum envio de relatórios de falha, nenhuma verificação de
  atualizações. **Ele não abre nenhuma conexão de rede.**
- O que ele guarda, guarda no seu computador, em arquivos que você pode ler e apagar.
- A Ordreth não recebe, não coleta e não mantém nenhum dado pessoal seu por meio do veil.

## De quem é

O veil é feito e publicado pela **Ordreth**, um estúdio de software independente ("nós"). A única forma de falar
conosco é **contact@ordreth.com**. Esta política cobre o programa veil e o que ele faz. Como o site da Ordreth e o
nosso endereço de e-mail tratam dados está descrito na página de privacidade do próprio site, e o GitHub, que
hospeda o repositório, os lançamentos e as discussões do veil, tem a sua.

## O que o veil lê

- **Os arquivos que você define como papel de parede**, com `veil set`, `veil daemon --input` ou o arquivo de
  configuração. O veil os lê para exibi-los; ele não os altera, não os move e não os copia.
- **O seu arquivo de configuração**, `~/.config/veil/config.toml`, se você escreveu um.
- **O que o seu compositor diz sobre os monitores e as janelas dele.** No Hyprland e no sway, o veil lê o IPC do
  compositor (qual monitor exibe uma janela em tela cheia, quais janelas o cobrem, qual workspace é exibido) para
  pausar um vídeo que ninguém vê. Ele não guarda nada disso depois de tomar a decisão.

## O que o veil escreve no seu computador

| O quê | Onde | O que contém |
|---|---|---|
| Os papéis de parede salvos | `~/.local/state/veil/state.json` (`$XDG_STATE_HOME`) | Por nome de monitor: o caminho do seu papel de parede, o modo de escala e a cor de fundo. Legível só por você (modo `0600`). |
| O socket de controle e o seu arquivo de trava | `$XDG_RUNTIME_DIR` (`veil-<display>.sock`) | Nada: o socket leva comandos dos seus próprios programas e some quando a sua sessão termina. |
| Um cache de plugins do GStreamer, só com o pacote do Nix | `~/.cache/veil/gstreamer-registry.bin` | A lista dos plugins do GStreamer instalados, para que a inicialização seja mais rápida. |
| O log | A saída de erro padrão, que o systemd põe no seu journal (`journalctl --user -u veil`) | O que o daemon fez: as versões do veil e do GStreamer, os caminhos dos papéis de parede, o decodificador que cada vídeo usa, os erros. O veil não escreve nenhum arquivo de log próprio. |

**Notificações de desktop.** Quando o daemon não consegue iniciar, não consegue desenhar um papel de parede ou
desiste de um vídeo que continua falhando, o veil mostra uma notificação pelo barramento da sessão (no máximo uma
por erro, com limite de frequência). O texto dela é o erro, que pode conter o caminho do papel de parede; o seu
daemon de notificações pode guardar um histórico disso.

`veil set --no-persist` exibe um papel de parede sem salvá-lo. Apagar `~/.local/state/veil/` e `~/.cache/veil/`
remove tudo o que o veil escreveu; o log segue a retenção do seu próprio journal.

## O que sai do seu computador

Nada, por parte do veil. Ele não tem código de rede; o GStreamer, que reproduz os vídeos, recebe só arquivos
locais.

O que você faz em torno do veil é tratado por outros:

- **Downloads.** Os tarballs de lançamento e o código-fonte são servidos pelo GitHub, e o flake do Nix é buscado
  no GitHub. O GitHub nos mostra quantas vezes cada arquivo de lançamento foi baixado, e nada sobre quem baixou.
- **Relatos e discussões** que você publica no GitHub são públicos, sob os termos do GitHub. Um relato de segurança
  vai pelo relato privado de vulnerabilidades do GitHub ou para contact@ordreth.com; usamos o que ele contém só
  para investigar o problema.

## Seus direitos

O veil não trata dados pessoais em nosso nome, então não há nada sobre você para mostrarmos, corrigirmos ou
apagarmos. Se você acha que ele trata, ou se você nos escrever, o endereço é contact@ordreth.com; a lei brasileira
(LGPD) se aplica ao que recebemos ali, e a lei do seu país onde ela lhe der mais direitos.

## Mudanças

Se o veil algum dia passar a fazer algo que esta página não descreve, a página muda primeiro, com a data acima, e a
mudança é registrada no [`CHANGELOG.md`](/apps/veil/docs/changelog).
