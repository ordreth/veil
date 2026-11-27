---
title: "Arquivo de configuração"
description: "Cada chave do config.toml e como ele se combina com veil set."
group: "Reference"
order: 4
source: "docs/configuration.md"
source_hash: "f6b0533ad5fd26e2c2a4d43056d6684cd2cf2b0115c3fddac13e154273d3be4f"
---

O `veil daemon` lê um arquivo TOML ao iniciar. O arquivo define as opções do daemon e dá a cada saída o seu papel de parede, de modo que uma configuração pode ser escrita uma só vez, nos dotfiles ou em um módulo do Nix, em vez de em uma linha de comando. Um exemplo comentado é [`contrib/config.example.toml`](https://github.com/ordreth/veil/blob/stable/contrib/config.example.toml). O pacote do Nix o instala como `share/doc/veil/config.example.toml`.

## Onde fica

| | |
|---|---|
| Padrão | `$XDG_CONFIG_HOME/veil/config.toml`, ou `~/.config/veil/config.toml` quando `XDG_CONFIG_HOME` não está definido (ou não é um caminho absoluto) |
| Outro arquivo | `veil daemon --config FILE`. O arquivo deve existir. |
| Sem arquivo | Não é um erro. Valem os padrões embutidos e os papéis de parede salvos. |

O daemon lê o arquivo uma única vez, quando inicia. **Uma mudança só tem efeito no próximo início** (`systemctl --user restart veil`, ou `veil quit` e `veil daemon`). Não há comando de recarga.

## Como verificar

```bash
veil config check                  # the default file
veil config check --config FILE
```

`veil config check` lê o arquivo como o daemon o leria. Não precisa de um daemon em execução. Ele abre cada `input` como o daemon faz ao iniciar: o arquivo deve existir e ser um vídeo ou uma imagem que possa ser decodificada. Em seguida, imprime com o que o daemon iniciaria:

```text
~/.config/veil/config.toml: ok
outputs: every output
mode: fill
bg_color: #000000
pause_on: fullscreen
release_unseen_after: after 2 s
[output."DP-3"]: video ~/Videos/waves.mp4 (fit, #000000)
[output."*"]: image ~/Pictures/wallpaper.png (fill, #000000)
```

Códigos de saída: `0` quando o arquivo está correto (ou quando não há arquivo), `2` quando o arquivo está incorreto, `1` quando o arquivo está correto, mas um papel de parede nele não pode ser exibido. Os erros vão para o stderr e informam o arquivo, a linha e a coluna:

```text
~/.config/veil/config.toml:7:1: unknown field `imput`, expected one of `input`, `mode`, `bg_color`
```

Um daemon que encontra o arquivo incorreto não inicia. Ele registra o mesmo erro no log, emite uma notificação de desktop e termina com código de saída 2, caso em que a unidade do systemd não o reinicia. Um papel de parede do arquivo que não pode ser exibido não interrompe o daemon: ele registra um aviso no log, e essa saída exibe a cor de fundo da seção.

## Chaves

Todas as chaves são opcionais. **Uma chave desconhecida é um erro**, de modo que uma chave escrita errada é informada em vez de ignorada. Há chaves apenas para o que o veil faz hoje. Versões futuras acrescentam chaves; não as renomeiam nem as removem.

### `[defaults]`

Estas chaves são as opções de linha de comando do daemon. Uma opção dada na linha de comando vale mais que o arquivo, e o arquivo vale mais que o padrão embutido.

| Chave | Tipo | Padrão | Opção | Significado |
|---|---|---|---|---|
| `mode` | string | `"fill"` | `--mode` | Como um papel de parede é ajustado à sua saída: `fill` (cobre, cortando o excesso), `fit` (por dentro, com barras em `bg_color`), `stretch` (ignora a proporção), `center` (tamanho nativo, centralizado), `tile` (tamanho nativo, repetido; `center` para um vídeo). Também é o padrão de `veil set`. |
| `bg_color` | string `"#rrggbb"` | `"#000000"` | `--bg-color` | Cor do que o papel de parede não cobre, e de uma saída sem papel de parede. Também é o padrão de `veil set` e `veil clear`. |
| `pause_on` | string | `"fullscreen"` | `--pause-on` | Quando um vídeo é pausado por não poder ser visto: `never`, `fullscreen` (uma janela em tela cheia na sua saída), `covered` (janelas cobrem a saída inteira, as translúcidas também). Funciona com Hyprland e sway. |
| `release_unseen_after` | inteiro ≥ 0 | `2` | `--release-unseen-after` | Segundos que um vídeo pode ficar sem que o compositor aceite seus quadros (uma tela bloqueada, um monitor desligado) antes de ser interrompido. Ele recomeça do início quando volta a ser exibido. É também por quanto tempo um vídeo cujo monitor sumiu (desconectado, desativado) é mantido, pausado, para continuar de onde estava quando o monitor voltar. `0`: nunca é interrompido, e é mantido até o monitor voltar. |
| `outputs` | array de strings | `[]` | `--output` (repetível) | As saídas em que desenhar, pelo nome. Vazio: todas as saídas, inclusive as conectadas depois. Quando dado na linha de comando, `--output` substitui esta lista. |

### `[output."NAME"]`

O papel de parede de uma saída, pelo nome que o compositor lhe dá (`veil outputs` as lista, por exemplo `DP-3` ou `HDMI-A-1`). As aspas só são necessárias para nomes que não são chaves TOML simples, como `"*"`.

| Chave | Tipo | Padrão | Significado |
|---|---|---|---|
| `input` | string | nenhum | Uma imagem ou um vídeo. O conteúdo decide qual dos dois, como em `veil set`. Veja "Caminhos" abaixo. |
| `mode` | string | `mode` de `[defaults]` | Como em `[defaults]`. Somente com um `input`. |
| `bg_color` | string `"#rrggbb"` | `bg_color` de `[defaults]` | Como em `[defaults]`. |

Uma seção sem `input` exibe apenas o seu `bg_color`, como depois de `veil clear`. Um `mode` em uma seção assim é um erro.

O `mode` e o `bg_color` da própria seção valem mais que os padrões, inclusive mais que um `--mode` ou `--bg-color` na linha de comando. Essas opções definem os padrões, e a seção é mais específica.

### `[output."*"]`

As mesmas chaves, para toda saída que não tem uma seção própria. Isso inclui as saídas conectadas depois. É o único padrão: `*` vale sozinho, e nomes como `DP-*` não são padrões.

## Caminhos

Em `input`:

- `~` no início, sozinho ou antes de `/`, é o seu diretório pessoal (`$HOME`). `~user` é um erro.
- `$NAME` e `${NAME}` são variáveis de ambiente do processo do daemon. Uma variável que não está definida é um erro. Escreva `$$` para um `$` literal.
- Um caminho relativo é tomado a partir do diretório do arquivo de configuração, como o arquivo foi nomeado. Um link simbólico não é seguido antes, então, com um arquivo ligado a partir do Nix store, o diretório é `~/.config/veil`.

No systemd, o ambiente do daemon é o do gerenciador de usuário, não o do seu shell. `$HOME` e as variáveis XDG estão lá; variáveis que o seu shell define podem não estar.

## O arquivo de configuração, `veil set` e os papéis de parede salvos

O veil guarda duas coisas que decidem o que uma saída exibe: este arquivo e o arquivo de estado do `veil set` (`$XDG_STATE_HOME/veil/state.json`, veja o README). Ao iniciar:

1. `veil daemon --input FILE` exibe `FILE` em todas as saídas. As seções do arquivo não são usadas.
2. Caso contrário, uma saída com uma seção própria exibe o papel de parede dessa seção.
3. Caso contrário, se há um `[output."*"]`, a saída o exibe.
4. Caso contrário, a saída exibe o seu papel de parede salvo: o último `veil set` para ela, ou para todas as saídas.
5. Caso contrário, ela exibe `bg_color`.

**O arquivo vence a cada início.** `veil set` muda o papel de parede de uma saída enquanto o daemon executa, e ele é salvo como de costume. Mas um daemon reiniciado exibe de novo o papel de parede do arquivo em todas as saídas que o arquivo cobre. Para manter um papel de parede de `veil set` entre reinicializações, deixe essa saída fora do arquivo: sem seção própria e sem `[output."*"]`. Os papéis de parede do arquivo também são salvos quando o daemon inicia, como acontece com `--input`, então um daemon iniciado depois sem o arquivo também os exibe.

Uma saída conectada enquanto o daemon executa recebe o mesmo tratamento, a menos que um `veil set` para todas as saídas tenha ocorrido desde o início. Esse `set` então decide, como acontece sem um arquivo.

## Exemplo

```toml
[defaults]
mode = "fill"
pause_on = "covered"

# The main monitor plays a video, letterboxed on dark grey.
[output."DP-3"]
input = "~/Videos/waves.mp4"
mode = "fit"
bg_color = "#202020"

# The laptop panel shows only a colour.
[output."eDP-1"]
bg_color = "#1e1e2e"

# Every other monitor, also ones plugged in later.
[output."*"]
input = "~/Pictures/wallpaper.png"
```

Com o home-manager, `xdg.configFile."veil/config.toml".text = ''...'';` grava o arquivo. Reinicie o serviço após uma mudança.
