# Divergências deliberadas entre `zpc` e `amxxpc`

O objetivo do `zpc` é **paridade de comportamento** com o `amxxpc`. Toda divergência
conhecida mora aqui. Se o oráculo diferencial (`scripts/difftest.mjs`) acusar diferença
num caso listado abaixo, **é esperado** — não é regressão.

Regra: uma divergência só entra nesta lista com (a) justificativa, (b) teste que a fixa,
e (c) avaliação de impacto em plugins reais. Qualquer outra diferença é bug do `zpc`.

## 1. Bugs do amxxpc que o `zpc` corrige

Estes são casos em que o compilador original trava, estoura a pilha ou tem código morto.
Corrigir é o ponto do projeto ("melhor que ele"), mas **muda a saída observável**.

| # | Caso | amxxpc | zpc | Justificativa |
|---|------|--------|-----|---------------|
| 1.1 | `#define A A+1` (macro auto-recursiva) | **Trava** (loop infinito em `substallpatterns`) | Erro 75 após 512 reescritas na mesma linha | Um compilador não pode travar com entrada válida-sintaticamente. O limite é alto o bastante para nenhuma macro real encostar. |
| 1.2 | Ciclo de `#include` | **Stack overflow** (sem guarda) | Fatal 102 ao passar de 32 níveis | Idem. Limite por *profundidade*, não por pertinência na pilha — headers reais legitimamente re-entram sob `#if defined _x_included`, e checar pertinência daria falso-positivo em todo o conjunto de includes do amxmodx. |
| 1.3 | `#else` múltiplo / `#elseif` depois de `#else` | **Silencioso** — o upstream testa `HANDLED_ELSE` mas nunca seta a flag, então os erros 60/61 são código morto | Emite 60/61 | O código do próprio compilador mostra a intenção; a flag simplesmente não é setada. Estamos ativando um diagnóstico que ele pretendia ter. |

**Impacto no oráculo**: um `.sma` que dispare 1.3 vai divergir. Nenhum plugin do corpus
oficial nem do corpus real dispara qualquer um dos três (todos são erros de programação
que impediriam o plugin de compilar/rodar).

## 2. Divergências por arquitetura

Diferenças que vêm de o `zpc` ter AST e passes separados, onde o original é single-pass.

| # | Caso | Diferença | Justificativa |
|---|------|-----------|---------------|
| 2.1 | Identificador indefinido dentro de `#if` | amxxpc emite erro 17; `zpc` trata como 0 em silêncio | No amxxpc o `#if` roda contra a tabela de símbolos completa (`const`/`enum` já visíveis). No `zpc` o preprocessador roda **antes** de qualquer análise semântica; reportar ali fabricaria "undefined symbol" em todo plugin real. Reavaliar quando/se o preproc ganhar acesso a constantes. |
| 2.2 | Colunas dentro de linha expandida por macro | amxxpc não tem spans; `zpc` tem spans exatos, exceto coluna dentro de expansão | `LineMap` mapeia linha→(arquivo, linha original). Substituição destrói a correspondência de bytes. Diagnósticos do próprio preprocessador não são afetados (carregam span exato). |
| 2.3 | `#line` | Parseado e validado, mas não renumera a saída | O `LineMap` já carrega as posições reais; renumerar seria redundante. |

## 3. Diferenças de saída binária (não são divergências semânticas)

| # | Caso | Nota |
|---|------|------|
| 3.1 | Bytes do `.amxx` | `zlib` (C) e `miniz_oxide` (Rust) podem emitir streams deflate diferentes no mesmo nível de compressão. **O oráculo deve comparar seções descomprimidas e campos de header, nunca bytes crus do arquivo.** |
| 3.2 | Endereços absolutos no bytecode | Comparar via disassembly normalizado (`zpc_asm::Style::Normalised`), que rotula alvos de salto e descarta endereços. Ver `crates/zpc-asm/src/disasm.rs`. |

## 4. Correções de premissa feitas durante a migração

Erros que estavam na documentação/plano deste repositório e foram corrigidos contra a fonte.
Não são divergências com o amxxpc — são casos em que **nós** estávamos errados.

| Premissa antiga | Realidade (verificada na fonte) |
|-----------------|--------------------------------|
| "continuação de linha é `\` ou `^`" (`AGENTS.md`) | Só `\`. `readline()` faz `if (*ptr=='\\')`, independente de `#pragma ctrlchar`. Um `^` no fim da linha é só um xor. |
| "portar `sp_symhash`" (plano, Fase B) | Arquivo **sem cabeçalho de licença**. Substituído por tabela clean-room com `HashMap`. |
| "`libpc300` é todo zlib" | `memfile.c/.h` é GPLv3; `sp_symhash`, `getch.h`, `sclinux.h` sem cabeçalho. Ver `docs/LICENSING.md`. |
| "o enum `OP_*` está em `amx.h`" | Não está nesta árvore. A numeração autoritativa é o `opcodelist[]` do `sc6.c`. |
| `AMX_HEADER` tem 60 bytes | 56, contando campo a campo do struct `PACKED` (`4+2+1+1+2+2 + 11×4`). |
| escapes `^ddd` são octais (como `\ddd` em C) | São **decimais**: `"^65;"` é `"A"`. |
