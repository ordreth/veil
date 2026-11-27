---
title: "Socket de controle e comandos"
description: "O protocolo do socket, cada comando, campo, código de erro e código de saída."
group: "Reference"
order: 5
source: "docs/ipc.md"
source_hash: "e0c23f711bf16c9e6fc0c29e85ccb9c1a916dadddfc22375c33ee183b681953c"
---

O `veil daemon` escuta em um socket Unix. Todo outro comando `veil` que fala com o daemon (`veil set`, `veil status`, ...) é um cliente desse socket, e qualquer outro programa também pode ser: uma barra de status, um script, um atalho de teclado, um programa em outra linguagem. Esta página é a referência do protocolo (o socket, o formato de transmissão, cada comando, cada campo, cada resposta e cada código de erro) e do lado da linha de comando (qual comando envia o quê, o que imprime, seu código de saída).

## O socket

O socket é `$XDG_RUNTIME_DIR/veil-$WAYLAND_DISPLAY.sock`:

* `XDG_RUNTIME_DIR` deve estar definido como um caminho absoluto. Não há alternativa em um diretório compartilhado como `/tmp`, onde outro usuário poderia criar o socket primeiro; sem ele, todo comando `veil` falha (código de saída 1).
* Se `WAYLAND_DISPLAY` é um caminho (alguns compositores definem o caminho completo do seu socket), só o seu último componente é usado. Se não está definido, o nome é `wayland-0`.

Assim, um daemon executa por compositor, e um cliente alcança o daemon do compositor em que executa. Em um shell:

```sh
sock="$XDG_RUNTIME_DIR/veil-${WAYLAND_DISPLAY##*/}.sock"
```

Acesso:

* O arquivo do socket tem o modo `0600`, e o daemon também verifica o id de usuário de cada cliente (`SO_PEERCRED`): um processo de outro usuário recebe a resposta `forbidden` e sua requisição não é lida. O `veil` faz o mesmo no sentido contrário e não envia requisições a um socket em que um processo de outro usuário escuta.
* Ao lado do socket fica o seu arquivo de lock, `veil-$WAYLAND_DISPLAY.sock.lock`. Um daemon mantém um lock sobre ele enquanto executa, e é assim que um segundo daemon sabe que já há um em execução (ele termina com código de saída 4). O arquivo de lock permanece quando o daemon termina; não o remova enquanto um daemon executa.
* O daemon remove o socket quando termina. Um daemon que foi morto o deixa para trás: a conexão é então recusada (`ECONNREFUSED`), o que o `veil` informa como "no daemon is running" (código de saída 3), e o próximo daemon remove o resto.

## Conexões

Uma conexão carrega uma requisição e uma resposta:

1. O cliente conecta e envia a requisição: um objeto JSON, em UTF-8, terminado por uma nova linha ou pelo fim do fluxo (o cliente encerra o seu lado de escrita). Espaços em branco ao redor do objeto são ignorados, então uma quebra de linha CRLF funciona.
2. O daemon lê até a primeira nova linha. Tudo depois dela é ignorado: uma segunda requisição na mesma conexão nunca é lida.
3. O daemon envia a resposta, um objeto JSON em uma linha terminada por uma nova linha, e fecha a conexão.

O `socat` faz tudo isso. (Os exemplos desta página foram enviados a um daemon em execução; só os caminhos, os nomes das saídas e números como o pid foram alterados.)

```sh
echo '{"cmd":"ping"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
```

A maioria das requisições é respondida de imediato. `set` e `clear` são respondidos quando o papel de parede está na tela, o que leva o tempo de carregar o arquivo (para um vídeo, até o primeiro quadro ser exibido); enquanto isso, o daemon continua atendendo outros clientes. Um cliente deve esperar pela resposta; o `veil` espera até 30 segundos. Se o cliente vai embora antes da resposta, a requisição ainda é executada.

### Limites

| Limite | Valor | Quando é ultrapassado |
|---|---|---|
| Tempo para enviar a requisição, a partir da conexão (mais dados não o estendem) | 2 s | `timeout`, e a conexão é fechada |
| Comprimento da linha da requisição, sem a nova linha | 64 KiB | `too_large` |
| Clientes conectados ao mesmo tempo, contando os que esperam a resposta de um `set` | 16 | `busy`, de imediato |
| Quanto tempo o `veil` espera por uma resposta | 30 s | o `veil` desiste (código de saída 1); o daemon ainda pode executar a requisição |

As respostas `timeout`, `too_large`, `busy` e `forbidden` vêm do transporte do daemon, antes de a requisição ser lida ou analisada; a conexão é fechada logo depois delas. Um cliente que ainda está enviando quando sua requisição é recusada pode ver a conexão ser reiniciada depois da resposta (o `socat` imprime `Connection reset by peer` depois de um `too_large`).

## Requisições

Uma requisição é um objeto JSON cujo `cmd` nomeia o comando, com os campos do comando ao lado:

```json
{"cmd":"set","output":"DP-3","input":"~/walls/sea.png","mode":"fit"}
```

* `cmd` é um dos comandos abaixo, em minúsculas.
* Um campo que o comando não aceita é recusado (`invalid_request`), inclusive para comandos que não aceitam campo algum: uma opção escrita errada nunca é ignorada em silêncio. Os campos podem vir em qualquer ordem.
* Um campo opcional pode ser omitido ou ser `null`; ambos significam o padrão.
* `output` nomeia uma saída (monitor) como o compositor a nomeia, como `DP-3` ou `HDMI-A-1` (`veil outputs` as lista). Não pode ser vazio. Sem ele, um comando vale para todas as saídas em que o daemon desenha, inclusive saídas desconectadas no momento (o daemon mantém o papel de parede delas para quando voltarem). Uma saída é uma em que o daemon desenha se ele a viu desde que iniciou ou, quando o daemon foi iniciado com `--output` (ou `outputs` no arquivo de configuração), se ela é uma das saídas nomeadas ali, conectada ou não. Qualquer outro nome recebe a resposta `unknown_output`, com os nomes em que o daemon desenha na mensagem.
* Uma cor (`bg_color`) é uma string `#rrggbb` de seis dígitos hexadecimais; o `#` pode ser omitido e os dígitos podem ser maiúsculos ou minúsculos. As respostas sempre escrevem `#rrggbb` em minúsculas.
* Um caminho (`input`) é absoluto. O daemon executa em um diretório de trabalho próprio, então recusa um caminho relativo, e `~` não é expandido. `veil set` transforma um caminho relativo em absoluto antes de enviá-lo.

### `ping`

Sem campos. Respondido com `ok` de imediato: há um daemon em execução e seu loop de eventos não está travado.

### `set`

Exibe uma imagem ou um vídeo como o papel de parede de uma saída ou de todas as saídas.

| Campo | Tipo | Padrão | Significado |
|---|---|---|---|
| `input` | string, obrigatório | | Caminho absoluto do arquivo. Se é uma imagem ou um vídeo é decidido pelo seu conteúdo (pela extensão apenas quando o conteúdo não é reconhecido); veja o README para os formatos. |
| `output` | string ou `null` | todas as saídas | A saída a alterar. |
| `mode` | [`mode`](#mode) | o `--mode` do daemon | Como o papel de parede é ajustado à saída. |
| `bg_color` | cor | o `--bg-color` do daemon | A cor do que o papel de parede não cobre (barras em `fit`, a área ao redor de `center`). |
| `persist` | booleano | `true` | `false`: exibe o papel de parede até o daemon parar, sem salvá-lo, de modo que um daemon reiniciado exibe o que foi salvo antes (`veil set --no-persist`). |

O que acontece:

* O arquivo é verificado e decodificado fora do loop de eventos do daemon, uma única vez para todas as saídas da requisição. Uma requisição é carregada por vez; as outras esperam em ordem.
* **A última requisição vence em cada saída.** Uma requisição toma suas saídas de todo `set` ou `clear` anterior que ainda não foi exibido. Uma requisição anterior que não tem mais saída alguma recebe a resposta `superseded` de imediato e nunca é exibida; uma que mantém algumas saídas (um `set` para todas as saídas seguido de um para uma única saída) é exibida nas saídas que manteve.
* A resposta é `ok` quando o papel de parede está em todas as saídas a que se destina: uma imagem quando é desenhada, um vídeo quando seu primeiro quadro está na tela, e em ambos os casos não antes de o vídeo que ele substitui ter saído. Uma saída desconectada, ou cujo vídeo é interrompido porque o compositor não o exibe (`released`), guarda o papel de parede para depois, e essa parte é concluída de imediato.
* Sem `output`, um papel de parede que foi exibido passa a ser o padrão: as saídas conectadas depois também o exibem, a menos que tenham um próprio.
* A menos que `persist` seja `false`, o papel de parede de cada saída é salvo em `$XDG_STATE_HOME/veil/state.json`, e um daemon iniciado sem `--input` o exibe de novo.

Erros:

* O próprio arquivo (não existe, não é um arquivo regular, está vazio, não pode ser lido, não é uma imagem nem um vídeo que o veil conheça, ou não pode ser decodificado): nada muda em nenhuma saída, e o código diz por quê: `not_found`, `unsupported_media` ou `failed` para os demais.
* Uma saída em que o papel de parede não pode ser exibido (um vídeo que falha ao iniciar ali): essa saída mantém o que exibia, as outras exibem o novo papel de parede, e a resposta é `failed`. Quando a requisição era para várias saídas, a mensagem nomeia as saídas que falharam (`DP-3: <reason>; HDMI-A-1: <reason>`), a menos que todas tenham falhado pelo mesmo motivo.
* Nenhuma saída onde exibi-lo (um daemon sem saídas, um `set` sem `output`): `failed`.

```sh
echo '{"cmd":"set","input":"~/walls/sea.png"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
echo '{"cmd":"set","output":"HDMI-A-1","input":"~/walls/sea.png","mode":"fit","bg_color":"#202020"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
echo '{"cmd":"set","output":"DP-3","input":"~/walls/waves.webm","persist":false}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
echo '{"cmd":"set","input":"~/walls/missing.png"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"not_found","message":"Failed to resolve path '~/walls/missing.png': No such file or directory (os error 2)"}
echo '{"cmd":"set","input":"~/notes.txt"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"unsupported_media","message":"'~/notes.txt' is neither an image nor a video that veil recognises, by its content or by its extension"}
echo '{"cmd":"set","output":"DP-9","input":"~/walls/sea.png"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"unknown_output","message":"output 'DP-9' is not driven by this daemon; it draws on DP-3, HDMI-A-1"}
echo '{"cmd":"set","input":"walls/sea.png"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"input must be an absolute path, got 'walls/sea.png'"}
echo '{"cmd":"set","input":"~/walls/sea.png","mode":"zoom"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"invalid request: unknown variant `zoom`, expected one of `fill`, `fit`, `stretch`, `center`, `tile`"}
```

Um `set` que um mais novo substituiu, enviado enquanto um vídeo ainda carregava:

```sh
echo '{"cmd":"set","input":"~/walls/waves.webm"}' | socat - UNIX-CONNECT:"$sock" &
echo '{"cmd":"set","input":"~/walls/sea.png"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"superseded","message":"replaced by a newer `veil set` before it was shown"}
# {"version":1,"status":"ok"}
```

### `clear`

Exibe apenas uma cor e solta o arquivo do papel de parede (`veil clear`). É um `set` sem arquivo: passa pela mesma fila (o último `set` ou `clear` vence em cada saída), é respondido quando a cor está na tela, passa a ser o padrão sem `output` e é salvo a menos que `persist` seja `false`, de modo que um daemon reiniciado exibe a cor também.

| Campo | Tipo | Padrão | Significado |
|---|---|---|---|
| `output` | string ou `null` | todas as saídas | A saída a limpar. |
| `bg_color` | cor | o `--bg-color` do daemon | A cor a exibir. |
| `persist` | booleano | `true` | Como em `set`. |

```sh
echo '{"cmd":"clear","output":"HDMI-A-1","bg_color":"#102030","persist":false}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
```

### `pause`

Pausa o papel de parede em vídeo de uma saída ou de todas as saídas até o `resume`, seja o que for que esteja na tela (`veil pause`). Respondido com `ok` de imediato.

| Campo | Tipo | Padrão | Significado |
|---|---|---|---|
| `output` | string ou `null` | todas as saídas | A saída a pausar. Deve ser uma para a qual o daemon tem um papel de parede: uma saída nomeada com o `--output` do daemon que nunca foi conectada recebe a resposta `unknown_output`. |

Um vídeo pausado mantém o quadro na tela e não decodifica nada. O daemon também pausa um vídeo por conta própria quando o compositor o oculta (`--pause-on`); os dois se somam, e um vídeo só é reproduzido quando nenhum dos dois o segura. Um papel de parede definido em uma saída pausada começa pausado. As imagens não têm o que pausar, mas são seguradas do mesmo modo (`paused_by_user` em `status`). Uma pausa não é salva, e uma saída conectada depois não é pausada.

```sh
echo '{"cmd":"pause","output":"DP-3"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
```

### `resume`

Encerra um `pause` (`veil resume`). Respondido com `ok` de imediato. Um vídeo que o compositor oculta continua pausado até voltar a ser exibido.

| Campo | Tipo | Padrão | Significado |
|---|---|---|---|
| `output` | string ou `null` | todas as saídas | A saída a retomar, como em `pause`. |

```sh
echo '{"cmd":"resume"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
```

### `toggle`

`resume` se todas as saídas a que se destina estão pausadas por `pause`; caso contrário, `pause` (em todas elas, de modo que um atalho de teclado transforma um estado misto em pausado). Respondido de imediato, com um resultado [`Toggled`](#toggled) que diz qual dos dois fez.

| Campo | Tipo | Padrão | Significado |
|---|---|---|---|
| `output` | string ou `null` | todas as saídas | A saída a alternar, como em `pause`. |

```sh
echo '{"cmd":"toggle"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok","result":{"paused":true}}
echo '{"cmd":"toggle"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok","result":{"paused":false}}
```

### `status`

Sem campos. Respondido de imediato, com um resultado [`Status`](#status): o daemon, seu papel de parede padrão e todas as saídas em que ele desenha, inclusive as que não estão conectadas. `veil status --json` imprime esse resultado.

```sh
echo '{"cmd":"status"}' | socat - UNIX-CONNECT:"$sock"
```

A resposta, em uma linha (espalhada aqui):

```json
{
  "version": 1,
  "status": "ok",
  "result": {
    "default": {"bg_color": "#000000", "input": "~/walls/sea.png", "mode": "fill"},
    "outputs": [
      {
        "connected": true,
        "hidden": false,
        "kind": "video",
        "name": "DP-3",
        "next": null,
        "paused_by_user": false,
        "scale": 1.0,
        "size": {"height": 1080, "width": 1920},
        "state": "playing",
        "video": {"dropped": 0, "fps": 30.0, "rendered": 41, "size": {"height": 720, "width": 1280}},
        "wallpaper": {"bg_color": "#000000", "input": "~/walls/waves.webm", "mode": "fill"}
      },
      {
        "connected": true,
        "hidden": false,
        "kind": "image",
        "name": "HDMI-A-1",
        "next": null,
        "paused_by_user": false,
        "scale": 1.0,
        "size": {"height": 1080, "width": 1920},
        "state": "shown",
        "video": null,
        "wallpaper": {"bg_color": "#202020", "input": "~/walls/sea.png", "mode": "fit"}
      }
    ],
    "pause_on": "fullscreen",
    "pid": 4242,
    "uptime_secs": 7385,
    "version": "1.0.0-beta.1"
  }
}
```

### `outputs`

Sem campos. Respondido de imediato, com um resultado [`OutputList`](#outputlist): as saídas que o compositor anunciou, se o daemon desenha nelas e, depois, as saídas que o daemon aguarda. `veil outputs --json` imprime esse resultado.

```sh
echo '{"cmd":"outputs"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok","result":{"outputs":[{"connected":true,"description":"Dell Inc. DELL U2720Q 1A2B3C4D","name":"DP-3","wallpaper":true},{"connected":true,"description":"Samsung Electric Company C24F390 HTQH600000","name":"HDMI-A-1","wallpaper":true}]}}
```

### `quit`

Sem campos. Encerra o daemon como o `SIGTERM`: ele responde `ok` primeiro; depois o daemon desmonta seus papéis de parede e termina com código de saída 0. O socket desaparece quando o daemon termina; `veil quit` espera por isso, então `veil quit && veil daemon` funciona. Sob a unidade do systemd, pare o serviço em vez disso (`systemctl --user stop veil`).

```sh
echo '{"cmd":"quit"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
```

### Requisições recusadas

```sh
echo '{"cmd":"explode"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"invalid request: unknown variant `explode`, expected one of `set`, `clear`, `pause`, `resume`, `toggle`, `status`, `outputs`, `quit`, `ping` at line 1 column 16"}
echo '{"cmd":"status","output":"DP-3"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"invalid request: unknown field `output`, there are no fields"}
echo '{"cmd":"set","input":"~/walls/sea.png","colour":"red"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"invalid request: unknown field `colour`, expected one of `output`, `input`, `mode`, `bg_color`, `persist`"}
echo '{"cmd":"clear","bg_color":"red"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"invalid request: invalid colour 'red': expected #rrggbb"}
echo '{"cmd":"pause","output":""}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"output must not be empty"}
echo 'not json' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"invalid request: expected ident at line 1 column 2"}
```

## Respostas

Uma resposta é `ok`, com um `result` para os comandos que informam alguma coisa:

```json
{"version":1,"status":"ok","result":{"paused":true}}
```

ou um erro, com um código para programas e uma mensagem para pessoas:

```json
{"version":1,"status":"error","code":"unknown_output","message":"output 'DP-9' is not driven by this daemon; it draws on DP-3, HDMI-A-1"}
```

### Campos

| Campo | Tipo | Significado |
|---|---|---|
| `version` | inteiro | A versão do protocolo do daemon, `1`. Veja [Compatibilidade](#compatibilidade). |
| `status` | `"ok"` ou `"error"` | Se a requisição foi executada. |
| `code` | string | Somente com `error`: que tipo de erro, um dos [códigos de erro](#códigos-de-erro). |
| `message` | string | Somente com `error`: o que deu errado, em inglês, para pessoas. Sua redação pode mudar; os programas olham o `code`. |
| `result` | objeto | Com `ok`, para `toggle` ([`Toggled`](#toggled)), `status` ([`Status`](#status)) e `outputs` ([`OutputList`](#outputlist)); omitido para os demais comandos. |

A ordem das chaves não significa nada (hoje, as chaves de um `result` vêm em ordem alfabética).

### Códigos de erro

| Código | Significado | Enviado para |
|---|---|---|
| `invalid_request` | JSON ou UTF-8 inválido, não é um objeto, um comando ou campo desconhecido, um campo do tipo errado ou um valor que não é aceito (um `input` vazio ou relativo, um `output` vazio, um `mode` desconhecido, uma cor que não é `#rrggbb`). | qualquer requisição |
| `too_large` | A linha da requisição é maior que 64 KiB. | qualquer requisição |
| `timeout` | Nenhuma requisição completa chegou em até 2 segundos após a conexão. | qualquer conexão |
| `forbidden` | O cliente executa como outro usuário. | qualquer conexão |
| `busy` | Já há 16 clientes conectados. | qualquer conexão |
| `superseded` | Requisições `set` ou `clear` mais novas tomaram todas as saídas desta antes de ela ser exibida. | `set`, `clear` |
| `unknown_output` | `output` nomeia uma saída em que o daemon não desenha; para `pause`, `resume` e `toggle`, também uma para a qual ele ainda não tem papel de parede, porque nunca foi conectada. | `set`, `clear`, `pause`, `resume`, `toggle` |
| `not_found` | O `input` não existe. | `set` |
| `unsupported_media` | O `input` não pode ser um papel de parede: não é um arquivo regular, está vazio, é um formato de imagem que o veil não decodifica (HEIF, JPEG XL), ou não é nem uma imagem nem um vídeo que o veil reconheça. | `set` |
| `failed` | Qualquer outra coisa: a requisição era válida, mas não pôde ser executada (um arquivo que não pode ser lido ou decodificado, um vídeo que falha ao iniciar, nenhuma saída onde exibi-lo); `message` diz por quê. | `set`, `clear`, `pause`, `resume`, `toggle`; em princípio, qualquer requisição que o daemon não possa responder |

## Resultados

Todo campo de um resultado está sempre presente, `null` quando não há nada a dizer, de modo que um programa não precisa distinguir uma chave ausente de uma vazia.

### `Status`

O resultado de `status`.

| Campo | Tipo | Significado |
|---|---|---|
| `version` | string | A versão do daemon (`veil --version`). |
| `pid` | inteiro | O id de processo do daemon. |
| `uptime_secs` | inteiro | Há quanto tempo o daemon executa, em segundos. |
| `pause_on` | [`pause_on`](#pause_on) | Quando um vídeo é pausado por estar oculto (`--pause-on`). |
| `default` | [`WallpaperInfo`](#wallpaperinfo) ou `null` | O que uma saída conectada depois exibe se não tem papel de parede próprio; `null`: a cor de fundo do daemon. |
| `outputs` | array de [`OutputStatus`](#outputstatus) | Todas as saídas em que o daemon desenha, inclusive as que não estão conectadas. |

### `OutputStatus`

Uma saída em um `Status`.

| Campo | Tipo | Significado |
|---|---|---|
| `name` | string ou `null` | O nome da saída; `null` para uma saída a que o compositor não deu nome. |
| `connected` | booleano | Se a saída está presente. Uma que não está mantém seu papel de parede para quando voltar. |
| `wallpaper` | [`WallpaperInfo`](#wallpaperinfo) | O que a saída exibe, ou exibirá quando for conectada. |
| `kind` | [`kind`](#kind) | Que tipo de papel de parede é esse. |
| `state` | [`state`](#state) | Em que ponto está o papel de parede. |
| `paused_by_user` | booleano | Seguro por `pause` (até o `resume`). |
| `hidden` | booleano | O compositor exibe sobre a saída algo que pausa um vídeo (`--pause-on`). |
| `size` | [`Size`](#size) ou `null` | O tamanho da superfície do papel de parede em pixels lógicos, enquanto ele tem uma. |
| `scale` | número ou `null` | A escala da superfície (`1.5` para 150 %), enquanto ela tem uma. Os pixels físicos são `size` vezes `scale`. |
| `video` | [`VideoInfo`](#videoinfo) ou `null` | O fluxo do vídeo, enquanto há um vídeo associado. |
| `next` | [`WallpaperInfo`](#wallpaperinfo) ou `null` | Um vídeo que está iniciando nesta saída e substitui o papel de parede atual quando seu primeiro quadro estiver lá. Um `set` que ainda está sendo carregado não aparece aqui. |

### `WallpaperInfo`

Um papel de parede: um arquivo ou apenas uma cor.

| Campo | Tipo | Significado |
|---|---|---|
| `input` | string ou `null` | O caminho absoluto do arquivo; `null` para uma cor sozinha (`clear`). |
| `mode` | [`mode`](#mode) | Como é ajustado à saída. |
| `bg_color` | string | A cor de fundo, `#rrggbb`. |

### `VideoInfo`

O fluxo de um vídeo e o que o compositor fez com seus quadros.

| Campo | Tipo | Significado |
|---|---|---|
| `size` | [`Size`](#size) ou `null` | O tamanho dos quadros decodificados, quando conhecido. |
| `fps` | número ou `null` | Quadros por segundo, quando conhecido, se o fluxo tem uma taxa fixa. |
| `rendered` | inteiro ou `null` | Quadros que o compositor aceitou, contados pelo sink de vídeo. A contagem recomeça a cada loop e a cada pausa. |
| `dropped` | inteiro ou `null` | Quadros que o compositor não aceitou a tempo, contados do mesmo modo. Muitos descartados e nenhum renderizado significa que o compositor não exibe o vídeo (uma tela bloqueada, um monitor desligado); o daemon então o interrompe (`released`). |

### `Size`

| Campo | Tipo | Significado |
|---|---|---|
| `width` | inteiro | Em pixels. |
| `height` | inteiro | Em pixels. |

### `OutputList`

O resultado de `outputs`.

| Campo | Tipo | Significado |
|---|---|---|
| `outputs` | array de [`OutputInfo`](#outputinfo) | As saídas que o compositor anunciou, depois aquelas que o daemon aguarda (nomeadas com `--output`, ou desconectadas com o papel de parede mantido). |

### `OutputInfo`

| Campo | Tipo | Significado |
|---|---|---|
| `name` | string ou `null` | `null` para uma saída a que o compositor não deu nome. |
| `description` | string ou `null` | O que o compositor diz que a saída é (marca e modelo), se disser algo. |
| `connected` | booleano | Se a saída está presente. |
| `wallpaper` | booleano | Se o daemon desenha um papel de parede nela: todas as saídas ou, com `--output`, as nomeadas. |

### `Toggled`

O resultado de `toggle`.

| Campo | Tipo | Significado |
|---|---|---|
| `paused` | booleano | `true`: as saídas agora estão seguras por `pause`; `false`: elas foram retomadas. |

## Valores

### `state`

Em que ponto está o papel de parede de uma saída (`OutputStatus.state`).

| Valor | Significado |
|---|---|
| `disconnected` | A saída não está conectada. |
| `starting` | O papel de parede está prestes a ser exibido: a superfície é nova, ou um vídeo espera pelo primeiro quadro. |
| `shown` | Uma imagem ou uma cor está na tela. |
| `playing` | Um vídeo é reproduzido. |
| `paused` | Um vídeo está pausado (`paused_by_user`, `hidden`), com seu quadro na tela. |
| `released` | Um vídeo foi interrompido porque o compositor não o exibe (uma tela bloqueada, um monitor desligado); ele recomeça do início quando volta a ser exibido. |
| `stopped` | Nada é exibido depois de um erro (o log do daemon diz qual); a próxima mudança da saída, ou o próximo `set`, tenta de novo. |

### `kind`

O que uma saída exibe (`OutputStatus.kind`).

| Valor | Significado |
|---|---|
| `color` | Apenas a cor de fundo. |
| `image` | Uma imagem. |
| `video` | Um vídeo (um GIF animado também é um). |

### `mode`

Como um papel de parede é ajustado à sua saída (`set`, `WallpaperInfo.mode`).

| Valor | Significado |
|---|---|
| `fill` | Cobre a saída inteira, mantendo a proporção; o que não cabe é cortado. O padrão do daemon. |
| `fit` | Mostra o papel de parede inteiro, mantendo a proporção; barras da cor de fundo preenchem o resto. |
| `stretch` | Cobre a saída inteira, distorcido para a proporção dela. |
| `center` | Um pixel do arquivo por pixel físico, centralizado; cortado ou cercado pela cor de fundo. |
| `tile` | Um pixel por pixel físico, repetido sobre a saída. Para um vídeo, age como `center`. |

### `pause_on`

Quando o daemon pausa um vídeo porque o compositor o oculta (`Status.pause_on`, o `--pause-on` do daemon). Exige um compositor cujo IPC o veil acompanhe (Hyprland, sway); nos demais, os vídeos sempre são reproduzidos.

| Valor | Significado |
|---|---|
| `never` | Nunca. |
| `fullscreen` | Uma janela em tela cheia está na saída. O padrão. |
| `covered` | Também quando janelas cobrem a saída inteira, as translúcidas inclusive. |

## Compatibilidade

* Novos comandos, campos, campos de resultado, códigos de erro e valores são acrescentados sem mudar `version`. Os campos nunca são renomeados nem removidos. `version` só é incrementada para uma mudança que um par da versão anterior leria errado.
* Um cliente deve ignorar os campos de resultado que não conhece, tratar um código de erro que não conhece como `failed`, e um `state` ou `kind` que não conhece como desconhecido. O próprio `veil` faz assim.
* Um daemon mais antigo que um comando o responde com `invalid_request`, com `unknown variant` na mensagem; um mais antigo que um campo, com `unknown field`. O `veil` então diz que o daemon é mais antigo que o cliente e deve ser reiniciado.
* Uma resposta sem `version` vem de um daemon mais antigo que o campo.

## Os comandos do `veil`

Os comandos do `veil` são os clientes acima, mais alguns que não precisam de daemon. Cada opção é descrita em `veil <command> --help` e nas páginas de manual (`man veil`, `man veil-set`, ...), que o binário gera a partir das mesmas definições.

### Comandos

| Comando | Requisição | Imprime no stdout |
|---|---|---|
| `veil daemon` | nenhuma: é o daemon | nada; o log vai para o stderr |
| `veil set --input FILE [--output NAME] [--mode MODE] [--bg-color COLOR] [--no-persist]` | `set`; um `FILE` relativo é tornado absoluto antes, `--no-persist` envia `"persist":false` | nada |
| `veil clear [--output NAME] [--bg-color COLOR] [--no-persist]` | `clear` | nada |
| `veil pause [--output NAME]` | `pause` | nada |
| `veil resume [--output NAME]` | `resume` | nada |
| `veil toggle [--output NAME]` | `toggle` | `paused` ou `playing` |
| `veil status [--json]` | `status` | uma linha por saída; com `--json`, o `result` em uma linha |
| `veil outputs [--json]` | `outputs` | uma linha por saída; com `--json`, o `result` em uma linha |
| `veil quit`, `veil stop` | `quit`, e depois espera (até 30 s) o daemon terminar | nada |
| `veil ping` | `ping` | `pong` |
| `veil config check [--config FILE]` | nenhuma: lê o arquivo de configuração como o daemon o leria | com o que o daemon iniciaria |
| `veil completions SHELL` | nenhuma | o script de autocompletar para `bash`, `zsh`, `fish` ou `nushell` |

`veil status` sem `--json`:

```text
veil 1.0.0-beta.1 (pid 4242), running for 2h 03m; videos pause on: fullscreen
default: ~/walls/sea.png (fill, #000000)
DP-3: playing ~/walls/waves.webm (fill, #000000); 1920x1080 at scale 1; video 1280x720, 30 fps, 41 frames shown, 0 dropped
HDMI-A-1: showing ~/walls/sea.png (fit, #202020); 1920x1080 at scale 1
```

O texto é para pessoas e pode mudar; os scripts usam `--json`, cujos campos são descritos em [Resultados](#resultados). Por exemplo, com `jq`:

```sh
veil status --json | jq -r '.outputs[] | "\(.name): \(.state)"'
```

Um comando que falha imprime seu erro no stderr. `veil set`, `clear`, `pause`, `resume`, `toggle` e `quit` também emitem uma notificação de desktop quando falham, já que costumam ser executados a partir de um atalho de teclado, onde ninguém vê o stderr; `status`, `outputs` e `ping` nunca emitem (uma barra de status pode perguntar a cada segundo enquanto nenhum daemon executa), e um `set` ou `clear` que foi `superseded` também não (o mais novo está na tela).

### Códigos de saída

Estes são estáveis: scripts podem confiar neles.

| Código | Significado |
|---|---|
| `0` | Sucesso. Para o daemon: ele foi parado com `SIGINT`, `SIGTERM` ou `quit`. |
| `1` | Qualquer outro erro, entre eles todo erro que o daemon responde (`superseded` inclusive). A mensagem está no stderr. |
| `2` | A linha de comando não pôde ser analisada, ou o arquivo de configuração não pode ser lido ou está incorreto (`veil daemon`, `veil config check`). |
| `3` | Um comando para um daemon em execução: nenhum daemon está em execução para este compositor (sem socket, ou um resquício em que ninguém escuta). |
| `4` | `veil daemon`: já há um daemon em execução para este compositor. Nada foi alterado. |
| `5` | `veil daemon`: o compositor desapareceu enquanto o daemon executava. |
