---
title: "Licença e termos"
description: "PolyForm Strict License 1.0.0 com permissões adicionais: de uso gratuito, código-fonte disponível, não é código aberto."
group: "Legal"
order: 15
source: "ADDITIONAL-PERMISSIONS.md"
source_hash: "a1c839f407f809410183c1adc8591ed1e44149c7c85423a1847cbc1c05eebe6f"
---

> Tradução para conveniência. Os textos que valem são os originais em inglês, `ADDITIONAL-PERMISSIONS.md` e `LICENSE` no repositório do veil; se esta tradução divergir deles, prevalece o inglês.

O veil é licenciado sob a [PolyForm Strict License 1.0.0](#o-texto-da-licença): você pode ler o código e usar o veil para qualquer fim não comercial, mas não distribuí-lo, alterá-lo nem criar novas obras a partir dele, nem usá-lo comercialmente. O licenciante é a Ordreth, um estúdio de software independente: o nome comercial com que os autores do veil o publicam (a Ordreth não é uma empresa registrada). O licenciante concede o que segue além dessa licença, para toda versão do veil lançada com este arquivo. Isso não amplia a licença de nenhuma outra forma.

## 1. Binários de lançamento e os arquivos neles

Você pode redistribuir os tarballs que o workflow de lançamento do veil publica na página GitHub Releases do projeto, **sem alterações** (byte a byte, com o `LICENSE`, este arquivo e todos os avisos que contêm), e pode montar um pacote próprio (por exemplo, um pacote `veil-bin` do AUR, ou um espelho) que baixa, verifica e instala os arquivos de um tarball assim, desde que eles não sejam modificados. Instalá-los em outros diretórios, e a mudança que o `install.sh` faz na linha `ExecStart` da unidade do systemd para que ela nomeie o binário instalado, não são modificações.

## 2. Compilar a partir do código-fonte para você

Você pode compilar o veil a partir do código-fonte (`cargo build`, `nix build`, um pacote de código-fonte que busca este repositório) e executar o que compilou. O resultado é para o seu próprio uso: sob esta seção, você não pode distribuí-lo.

## 3. Citações

Você pode citar trechos curtos do código-fonte (até 50 linhas por vez), com atribuição, em um relato de bug, uma discussão, um artigo ou uma resenha.

## 4. Nome e logotipo

O nome "veil" e o ícone em `assets/icon/` podem ser usados para se referir a este software (na listagem de um pacote, em um artigo, em um relato de compatibilidade), e não como nome ou logotipo de outro produto, de uma versão modificada ou de uma obra derivada dele. Os arquivos do ícone estão sob a mesma licença que o código.

## O que não é concedido

Uso comercial, cópias modificadas, distribuição do código-fonte e qualquer outra coisa que a licença não permita exigem a permissão por escrito do licenciante. Peça em contact@ordreth.com.

## Mudanças

O licenciante pode mudar a licença ou estas permissões para versões futuras. Os termos de uma versão já lançada continuam como foram lançados.

---

## O texto da licença

Required Notice: Copyright (c) 2026 Ordreth (https://github.com/ordreth/veil)

As permissões adicionais em ADDITIONAL-PERMISSIONS.md somam-se à licença abaixo. O texto da licença em si é a PolyForm Strict License 1.0.0, sem alterações.

### PolyForm Strict License 1.0.0

<https://polyformproject.org/licenses/strict/1.0.0>

#### Aceitação

Para obter qualquer licença sob estes termos, você deve concordar com eles tanto como obrigações estritas quanto como condições de todas as suas licenças.

#### Licença de direitos autorais

O licenciante concede a você uma licença de direitos autorais sobre o software para fazer, para qualquer finalidade permitida, tudo o que você poderia fazer com o software e que de outro modo violaria os direitos autorais do licenciante sobre ele, exceto distribuir o software ou fazer alterações ou novas obras baseadas no software.

#### Licença de patentes

O licenciante concede a você uma licença de patentes sobre o software que abrange as reivindicações de patentes que o licenciante pode licenciar, ou venha a poder licenciar, e que você violaria ao usar o software.

#### Finalidades não comerciais

Qualquer finalidade não comercial é uma finalidade permitida.

#### Usos pessoais

O uso pessoal para pesquisa, experimentação e testes em benefício do conhecimento público, estudo pessoal, entretenimento privado, projetos de hobby, atividades amadoras ou observância religiosa, sem nenhuma aplicação comercial prevista, é uso para uma finalidade permitida.

#### Organizações não comerciais

O uso por qualquer organização beneficente, instituição de ensino, organização pública de pesquisa, organização de segurança pública ou de saúde, organização de proteção ambiental ou instituição governamental é uso para uma finalidade permitida, independentemente da fonte de financiamento ou das obrigações resultantes do financiamento.

#### Uso legítimo

Você pode ter direitos de "uso legítimo" (*fair use*) sobre o software segundo a lei. Estes termos não os limitam.

#### Nenhum outro direito

Estes termos não permitem que você sublicencie ou transfira qualquer uma das suas licenças a outra pessoa, nem impedem o licenciante de conceder licenças a outras pessoas. Estes termos não implicam nenhuma outra licença.

#### Defesa de patentes

Se você fizer qualquer alegação por escrito de que o software viola ou contribui para a violação de qualquer patente, sua licença de patentes sobre o software concedida sob estes termos termina imediatamente. Se a sua empresa fizer tal alegação, sua licença de patentes termina imediatamente para o trabalho em nome da sua empresa.

#### Violações

Na primeira vez que você for notificado por escrito de que violou qualquer um destes termos, ou fez com o software algo não coberto pelas suas licenças, suas licenças podem continuar mesmo assim se você passar a cumprir integralmente estes termos e tomar medidas práticas para corrigir as violações passadas, dentro de 32 dias do recebimento da notificação. Caso contrário, todas as suas licenças terminam imediatamente.

#### Nenhuma responsabilidade

***Na medida em que a lei permitir, o software é fornecido como está, sem qualquer garantia ou condição, e o licenciante não será responsável perante você por quaisquer danos decorrentes destes termos ou do uso ou da natureza do software, sob qualquer tipo de pretensão jurídica.***

#### Definições

O **licenciante** é a pessoa física ou jurídica que oferece estes termos, e o **software** é o software que o licenciante disponibiliza sob estes termos.

**Você** se refere à pessoa física ou jurídica que concorda com estes termos.

**Sua empresa** é qualquer pessoa jurídica, empresa individual ou outro tipo de organização para a qual você trabalha, mais todas as organizações que controlam essa organização, são controladas por ela ou estão sob controle comum com ela. **Controle** significa a propriedade de substancialmente todos os ativos de uma entidade, ou o poder de dirigir sua administração e suas políticas por voto, contrato ou outro meio. O controle pode ser direto ou indireto.

**Suas licenças** são todas as licenças concedidas a você sobre o software sob estes termos.

**Uso** significa qualquer coisa que você faça com o software que exija uma das suas licenças.
