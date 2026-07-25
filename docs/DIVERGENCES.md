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
| 1.4 | Divisão constante por zero (`new x[1/0]`) | **Crash (SIGFPE)** — `calc()` não tem guarda nenhuma | Erro 29 e expressão vira não-constante | Um compilador não pode morrer com entrada do usuário. **Isto é invenção nossa**: o upstream não tem diagnóstico aqui, ele simplesmente morre. O 29 é o código que o próprio `calc()` usa para "isto nunca deveria acontecer". |

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
| 2.6 | ~~`#pragma ctrlchar` tratado como valor único~~ **CORRIGIDO** | O preprocessador registra `(linha, caractere)` e o `Scanner::scan_with_ctrl_changes` aplica cada troca ao cruzar a linha, reproduzindo a natureza posicional do amxxpc. Mantido aqui como registro: tratar o ctrlchar como valor único fazia um `#pragma ctrlchar '\'` no plugin reler retroativamente todos os headers escritos para `^`. |
| 2.4 | Warning 208 (função com tag usada antes da definição) | amxxpc **reparseia o arquivo inteiro**; o `zpc` avisa na definição e segue | Conjunto aceita/rejeita é o mesmo (208 é warning). Mas a *tag* das chamadas feitas antes da definição só fica correta na segunda passada do upstream — checar tag nesses call sites fica explicitamente a cargo do chamador. |
| 2.5 | Warnings 203/204 (símbolo não usado) | amxxpc emite no teardown de cada escopo; o `zpc` acumula e ordena por `(span, código)` | Ordem de saída passa a ser função da fonte, não da ordem de iteração de `HashMap`. Teste roda o mesmo escopo de 64 símbolos seis vezes e exige saída byte-idêntica. |

## 2b. Comportamento não especificado no upstream

Casos em que o C é UB ou depende da plataforma. O `zpc` fixa um comportamento; se algum dia
o oráculo divergir aqui, é porque o binário de referência escolheu outro.

| # | Caso | Escolha do `zpc` | Base |
|---|------|------------------|------|
| 2b.1 | Deslocamento com contagem fora de `0..31` | Mascara com `& 31` | UB em C. O amxxpc compila para shift x86, que mascara em 5 bits — igual ao interpretador AMX. |
| 2b.2 | Overflow em aritmética constante | Wrapping silencioso, **sem diagnóstico** | Verificado: o amxxpc **não** diagnostica overflow constante (o erro 105 só cobre dimensão de array > `INT_MAX`, e está dentro de `#if INT_MAX < LONG_MAX`, morto em build 32-bit). É overflow de `int` do C, que na prática dá wrap. Inclui `i32::MIN / -1`, que em Rust entraria em pânico. |
| 2b.3 | Rational de ponto fixo (`#pragma rational` com dígitos) | Escala por `10^digits` com arredondamento | O AMXX nunca distribui build de ponto fixo; leitura razoável do `sc2.c`, **não validada** contra binário real. |

### Semântica verificada que é fácil errar

Não são divergências — são regras do Pawn que uma implementação ingênua quebraria. Ficam
registradas porque custaram investigação:

- **Divisão é floored, resto acompanha o sinal do divisor** (`truemodulus` em `sc3.c:632`):
  `-7/2 == -4`, `-7%2 == 1`, `7/-2 == -4`, `7%-2 == -1`. Não é a truncagem do C **nem** o
  `rem_euclid` do Rust (que difere quando o divisor é negativo).
- **Ternário nunca é constante**: `hier13()` seta `iEXPRESSION` incondicionalmente, então
  `1 ? 2 : 3` não dimensiona array — e ainda dispara 206.
- **`&&`/`||` não fazem short-circuit em tempo de fold**: `skim()` exige todos os operandos
  constantes, logo `0 && f()` é expressão de runtime. Só o codegen curto-circuita.
- **206 vs 205**: condição constante dispara **206 se não-zero**, **205 se zero**. Dois sítios:
  condição de `if`/`while`/`do`/`for` (`test()`) e condição de ternário (`hier13`).
- **`charsmax`/`cellsof` não são tokens do compilador** — são `#define` nos headers do AMXX que
  expandem para expressões com `sizeof`. Chegam já expandidos; não há nada a implementar.
- **`tagof` devolve `tag | PUBLICTAG`** (`0x80000000`), não o id de tag puro.
- **Aritmética rational não dobra**: `1.5 + 2.5` cai em `check_userop` (o `float.inc` define
  `operator+(Float:,Float:)`) e vira `iEXPRESSION`. O único fold rational é o `-` unário.

## 2c. Onde o *linter* (não o `zpc`) é deliberadamente mais permissivo

O `zpc` persegue paridade. O linter heurístico do zplint tem outro objetivo — não incomodar
com código inofensivo. Onde os dois discordam de propósito, fica registrado aqui.

| Caso | amxxpc / `zpc` | linter `zplint` | Por quê |
|------|----------------|-----------------|---------|
| `set_task(0, "x")` — literal `0` num parâmetro `Float:` | **Avisa 213.** `matchtag()` compara apenas tags; não existe exceção para zero em `matchtag`/`checktag`/`callfunction`. Untagged→tagged nunca coage (a coerção exige `formaltag == 0`). | `api_tag_int_arg` **não** avisa | O padrão de bits de `0` *é* `0.0`, então o código roda certo. Avisar aqui seria ruído em código correto. **É escolha do linter, não comportamento do compilador** — a doc antes afirmava o contrário e foi corrigida. |

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
| limite de nome de símbolo é 31 | **63** — o AMXX aumentou `sNAMEMAX` em relação ao Pawn original. |
| `foo()` sem `foo` definido dá erro 17 | Dá **erro 4** ("function is not implemented"). O 17 é para referência que não é chamada. |
| o enum `OP_*` tem aridade inferível pelo nome | Vem da função de emissão que cada linha do `opcodelist[]` referencia (`parm0`/`parm1`/`parm2`); `casetbl` é variável (`2+2n` células). |
| função retorna array como `Float:make()[3]` (afirmado na doc da AST) | **Errado.** `newfunc()` (definição) vai direto para `if (!matchtoken('('))` — não tem loop de dims. Só `funcstub()` (native/forward) tem `while (matchtoken('['))`, e **antes** do nome: `native Float:[3] make_vec();`. Logo `return_dims` só é populado para native/forward. |


---

## 5. Os 2 plugins oficiais que o `zpc` não compila — e por quê

`ts/stats.sma` e `ts/stats_logging.sma` falham com 14× erro 017 (`create_entity`,
`DispatchKeyValue` indefinidos).

**Não é bug do `zpc`.** `tsx.inc` usa essas natives mas inclui apenas `tsstats`;
nem ele nem os dois plugins incluem `engine.inc`, onde elas são declaradas
(`engine.inc:615`). O header é auto-insuficiente como distribuído.

**Prova**: adicionar uma única linha `#include <engine>` a cada plugin faz ambos
compilarem sem nenhum erro. Ou seja, todo o resto desses plugins — que são dos
maiores do conjunto — o `zpc` já processa corretamente.

O amxxpc real falharia da mesma forma com este conjunto de includes. **Não
verificável sem o binário de referência**: é possível que o build oficial do AMXX
passe `-i` adicional ou que o módulo TS distribua um include próprio.

Se essa leitura estiver certa, o `zpc` compila **72 de 72** plugins oficiais
compiláveis.
