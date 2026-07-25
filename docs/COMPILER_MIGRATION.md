# zpc — Reimplementação do compilador AMXX Pawn em Rust

> Plano de migração do `amxxpc` (C, ~23k LOC, parado há ~5 anos) para um compilador
> Rust moderno embutido no zplint.
>
> **Status: EM ANDAMENTO.** Ver a tabela de progresso na §8. O oráculo diferencial
> ainda **não roda** (não há `amxxpc.exe` nesta máquina), então nenhuma paridade está
> provada — só os testes próprios passam. Divergências deliberadas e correções de
> premissa ficam em `docs/DIVERGENCES.md`.

## 0. Objetivo e não-objetivos

**Objetivo**: `zplint compile plugin.sma → plugin.amxx`, com diagnósticos ≥ amxxpc e
saída **comportamentalmente idêntica** (carrega e executa igual no runtime AMXX). Internamente
moderno: AST, Rust seguro (sem os overflows do C), erros com span/caret, saída determinística.

**Não-objetivos (inicialmente)**: células 64-bit (AMXX é 32-bit), alvos exóticos, compatibilidade
com forks SA-MP (warnings 235-240). Substituir o amxxpc só faz sentido depois de paridade total.

## 1. Licenciamento (resolver ANTES de codar)

| Componente | Origem | Licença | Ação |
|-----------|--------|---------|------|
| `libpc300` (sc1-7, núcleo ~12k LOC) | CompuPhase Pawn | **zlib permissiva** | Portar livremente. Marcar como "altered version", creditar origem, manter aviso. |
| Empacotador `.amxx` (amxxpc.cpp, Binary.cpp) | AMX Mod X | **GPLv3** | **Não portar.** Reimplementar o formato da spec (magic+versão+cellsize+zlib). |
| Tabela de erros (`sc5-in.scp`) | CompuPhase | zlib | Reusar textos p/ paridade. |
| `memfile.c/.h` — **dentro do libpc300** | AMX Mod X | **GPLv3** | **Não portar.** Usar `Vec<u8>`/`Cursor`. |
| `sp_symhash.c/.h` — **dentro do libpc300** | AlliedModders | **sem cabeçalho de licença** | **Não portar.** Usar `HashMap`. |

- ✅ `ATTRIBUTION.md` (raiz) — créditos a ITB CompuPhase + AMXX Dev Team, aviso zlib
  reproduzido, marcação de "versão alterada", e separação entre *portado* e *reimplementado*.
- ✅ `docs/LICENSING.md` — tabela por componente, regra "GPL nunca é transcrito" e checklist
  para contribuidores que adicionam código portado. **Ler antes de portar qualquer arquivo.**
- ⚠️ **Atenção**: `libpc300` **não** é 100% zlib (ver as duas linhas novas na tabela). Conferir
  o cabeçalho de cada arquivo individualmente, nunca presumir pela pasta.
- **Bloqueador de compliance**: nada de copiar trechos do amxxpc.cpp (GPL) para o zpc.
- 🔓 **Em aberto (decisão do dono)**: o zplint não tem `LICENSE` nem `license` no `Cargo.toml`.
  A zlib é compatível com qualquer escolha (MIT/Apache/GPL/proprietária) — detalhes e
  consequências em `docs/LICENSING.md` §5.

## 2. Oráculo diferencial (construir ANTES de portar — passo #1 de derisk)

O amxxpc é a verdade-base. Todo o projeto é validado por **teste diferencial**.

- Obter `amxxpc.exe` de referência + includes empacotados (o usuário já tem AMXX instalado).
- Corpus: 74 plugins oficiais + 542 do corpus real + fixtures de borda escritas à mão.
- Para cada `.sma`: rodar amxxpc → capturar `(diagnósticos stdout, exit code, bytes .amxx)`.
- `zpc` precisa: **bater os diagnósticos exatamente** e produzir `.amxx` comportamentalmente idêntico.
- Byte-idêntico é meta-esticada; oráculo primário = **igualdade de disassembly normalizado**
  (desmontar ambos os `.amx`, normalizar endereços/símbolos, comparar stream de opcodes) +
  igualdade de diagnósticos.
- Harness: `cargo test --test diff` roda o corpus inteiro e falha em qualquer divergência.

## 3. Arquitetura (modernizada, não transcrição literal do C)

- Workspace com crates: `zpc-lex`, `zpc-parse` (AST), `zpc-sema` (tags/const), `zpc-codegen`,
  `zpc-asm`, `zpc-amxx`. O binário `zplint` passa a depender de `zpc`.
- **Decisão-chave: AST real** (o original é single-pass). Ganho: diagnósticos melhores,
  incremental, testável. Custo: reproduzir semântica single-pass (forward refs, ordem de
  primeira-ocorrência) via coleta de símbolos em 2 passadas.
- Saída determinística (ordenação estável de símbolos) — melhora sobre o C.
- Diagnósticos com span/caret (ganho de DX), mas **códigos E/W e textos espelham `sc5-in.scp`**
  para paridade com o oráculo.
- **Modernização de maior valor**: o *linter* e o *compilador* passam a compartilhar
  lexer+parser+AST. O zplint deixa de ser heurístico (regex) e vira AST-based — elimina de vez
  a classe de falso-positivo/negativo das heurísticas atuais.

## 4. Fases (cada uma travada pelo oráculo)

### Fase A — Lexer + Preprocessador  (porta `sc2.c`) · ~3-5 sem
- Tokens; números (hex/bin/char, escapes `^`, `#pragma ctrlchar`); strings (continuação
  multi-linha `\`/`^`); operadores.
- Preprocessador: `#include`/`#tryinclude` (resolução de path), `#define` objeto+função e
  expansão, `#if/#elseif/#else/#endif/#assert/#error/#undef`, `#pragma` (dynamic, semicolon,
  tabsize, ctrlchar, deprecated, library, reqclass, compress, pack, rational, amxlimit, codepage,
  unused), símbolos predefinidos (`__LINE__`, `__DATE__`, …).
- i18n/codepage (`sci18n`) — provavelmente simplificável/adiável.
- **Oráculo**: paridade do dump preprocessado; senão, harness de paridade de tokens.

### Fase B — Parser + tabela de símbolos → AST  (porta lógica de decl. do `sc1.c`) · ~4-6 sem
- `new/static/stock/public/native/forward/const`; `enum` (com passo `(<<= 1)`, tagueado);
  arrays (multi-dim, indeterminados, tamanho por expressão); headers de função; args default;
  `&` by-ref; rest args `...`; operadores/tags de usuário; `#pragma unused`; states (`scstate`).
- Tabela de símbolos: **clean-room** com `HashMap` + escopos. **Não portar `sp_symhash.c/.h`** —
  não tem cabeçalho de licença, procedência indefinida (ver `docs/LICENSING.md`). Reimplementar
  a partir do comportamento observável, nunca do código.
- **Oráculo**: dump de símbolos + paridade de erros (redefinição, indefinido, etc.).

### Fase C — Semântica / sistema de tags  (porta `sc3.c`) · ~5-8 sem · **núcleo do "100% de validação"**
- Parser de expressões (precedência); regras de coerção de tag; constant folding;
  `sizeof/tagof/charsmax/cellsof`; regras de indexação de array; regras de atribuição
  string/array; **conjunto completo de warnings 200-234**.
- **Oráculo**: paridade de diagnósticos em todo o corpus. **Marco que entrega a validação real.**

### Fase D — Geração de código  (porta `sc4.c` + emissores) · ~5-8 sem
- Statements (`if/while/do/for/switch/case/return/break/continue/goto`); stack frames; modos de
  endereçamento; índices de native/public; stream de assembly-texto compatível com o assembler.
- **Oráculo**: paridade de disassembly normalizado.

### Fase E — Assembler + peephole  (porta `sc6.c` + `sc7.c`) · ~4-6 sem
- Assembly-texto → bytecode AMX; relocação; tabela de natives; tabela de publics; passes de
  otimização peephole (necessários p/ paridade de saída).
- **Oráculo**: diff de bytecode `.amx` (nível de byte após normalização).

### Fase F — Container .amxx + debug info  (reimpl. da spec, não porta GPL) · ~2-3 sem
- `.amx` → `.amxx`: header (magic/versão/cellsize=4), compressão zlib, seção única 32-bit.
- Geração de debug info (`.dbg`, `amxdbg`) p/ números de linha em erros de runtime.
- **Oráculo**: runtime AMXX carrega e roda; comparar comportamento com o `.amxx` do amxxpc.

### Fase G — Integração + modernização · ~3-5 sem
- Subcomandos `zplint compile`/`zplint build`; mapeamento de erros p/ a saída Biome do zplint.
- **Unificar linter+compilador na mesma AST** (grande upgrade de qualidade do linter).
- Modernizar: diagnósticos ricos, compile `--watch`, servidor LSP, multi-arquivo paralelo.

## 5. Esforço e sequenciamento por valor

Total ~**7-11 pessoa-meses** para paridade completa (A-F) + modernização. Sequência que
**adianta valor**:

- **M1 (Fases A-C)** — *paridade de diagnósticos* = a meta "100% de validação", **sem** codegen.
  Entregar primeiro como modo profundo do `zplint check`. ~3-4,5 meses.
- **M2 (Fases D-F)** — produzir `.amxx` executável. ~3-4,5 meses.
- **M3 (Fase G)** — unificar linter+compilador na AST, DX moderna. ~1 mês.

M1 sozinho já resolve o que o usuário vem perseguindo (validação real), sem depender de gerar
bytecode — bom ponto de corte se o escopo precisar encolher.

## 6. Riscos

| Risco | Mitigação |
|-------|-----------|
| Paridade de saída é o mais difícil (ordem de opt, layout de símbolos) | Oráculo de comportamento (não de byte); corpus grande |
| Compat bug-por-bug (plugins dependem de quirks) | Corpus + teste diferencial contínuo |
| Scope creep | Paridade PRIMEIRO, melhorias DEPOIS |
| Manutenção vs upstream | Upstream ~dormente (5 anos) → risco menor; ainda assim rastrear |
| Banda de um mantenedor só | Sequência M1→M2→M3 entrega valor cedo |

## 7. Próximos passos (semana 1)

1. Nota de licença/atribuição (`ATTRIBUTION.md`) + definir licença do zplint.
2. Levantar o harness diferencial + `amxxpc.exe` de referência + baseline do corpus.
3. Scaffold do workspace `zpc`.
4. Iniciar Fase A (lexer).

---

## 8. Progresso real

Contagem de testes verificada com `cargo test --workspace`. "Parcial" significa que o
que existe é testado e está no repositório, não que a fase esteja fechada.

| Fase | Crate(s) | Estado | Testes |
|------|----------|--------|--------|
| A — Lexer + Preprocessador | `zpc-lex` | ✅ scanner + preproc completos | 97 |
| B — Parser + símbolos | `zpc-parse` | ✅ decl + stmt + expr; fixture parseia com 0 diagnósticos | 110 |
| C — Semântica / tags | `zpc-sema` | ✅ símbolos + tags + constant folding | 103 |
| D — Codegen | `zpc-codegen` | 🟡 em construção | — |
| E — Assembler + peephole | `zpc-asm` | 🟡 opcodes + disassembler prontos; assembler em construção | 16 |
| F — Container `.amxx` | `zpc-amxx` | ✅ read/write + header AMX | 22 |
| G — Integração | `zplint` | 🟡 `zplint disasm` funcional; `compile` falta | 104 |
| — | `zpc-diag` / `zpc-ast` | ✅ 136 diagnósticos + AST completa | 12 |

**Total: 464 testes, clippy limpo.**

**Falta**: codegen (Fase D), assembler (Fase E) e o driver que amarra tudo em
`zplint compile`. As fases A-C — que é onde mora a "validação 100%" — estão construídas
e testadas, mas **não validadas contra o oráculo** (ver bloqueadores).

### Infraestrutura de validação já de pé
- `scripts/difftest.mjs` — oráculo diferencial (precisa de `amxxpc.exe`).
- `crates/zpc-asm/src/disasm.rs` — disassembly normalizado (independente de layout),
  que é a forma correta de comparar saída, já que bytes de `.amxx` divergem por zlib.
- `crates/zpc/tests/fixtures/` — Pawn escrito à mão cobrindo as armadilhas conhecidas.

### Fase G — caminho de lint sobre a AST (primeiro corte)

`src/ast_lint.rs` reusa `zpc-lex` + `zpc-parse` e responde 3 regras a partir da
árvore: `empty_statement`, `string_literal_compare`, `comparison_as_statement`
(mesmos `rule_id`, mesma severidade, mesmo `rules.disable`). Ligado por padrão,
desligável com `[rules] ast = false`.

- **Falha de parse nunca vira erro de lint.** `ast_lint::lint` devolve `None` e o
  arquivo inteiro cai no motor regex. `#include` é apagado antes do preproc (o
  diagnóstico 100 é *fatal* e truncaria o arquivo).
- **Taxa de parse limpo**: 48/74 (64,9%) nos plugins oficiais, 1428/1630 (87,6%)
  no corpus real.
- **`zplint ast-compare <arquivos>`** roda os dois caminhos e lista divergência
  por regra/linha. Hoje: 0 divergências nos dois corpora, contagens idênticas.
- `else_paren` **não** migrou: `else (cond) { }` não parseia (erro 1 + 54), então
  a AST nunca veria o bug. Volta a ser candidata quando o parser tiver recuperação
  de erro melhor.

### Bloqueadores
1. **Sem `amxxpc.exe`** → o oráculo não roda. É o item de maior risco do projeto:
   sem ele, "paridade" é afirmação não verificada.
2. **Licença do zplint indefinida** → ver `docs/LICENSING.md` §5.


---

## 9. Resultado medido (fim da sessão de migração)

`zplint compile` nos 74 plugins oficiais do `alliedmodders/amxmodx`:

| | início | fim |
|---|---|---|
| plugins que compilam | **0 / 74** | **72 / 74 (97%)** — os 2 restantes são um header auto-insuficiente, ver abaixo |
| erros de compilação | 8476 | **14**, todos do mesmo `tsx.inc` |
| testes do workspace | 555 | 649, 0 falhas |
| baseline do linter | 0 erros | 0 erros (mantido) |

### As 10 causas-raiz removidas

Cada uma diagnosticada na fonte C, com o erro que dominava antes:

| # | Causa | Erro | Impacto |
|---|-------|------|---------|
| 1 | Quebra de linha tratada como fim de protótipo → todo corpo sem chaves quebrava | 010 | 1585 → 0 |
| 2 | Operadores definidos pelo usuário sem despacho (+ propagação de tags) | 007 | 1947 → 0 |
| 3 | Constantes predefinidas do `setconstants()` ausentes | 017 | −1135 |
| 4 | `Usage::DEFINED` não setado → `sizeof` rejeitava variável válida | 008/017 | −958 |
| 5 | Locais nunca entravam na symbol table → `charsmax` falhava | 008/017 | −2836 |
| 6 | Sub-array como argumento (`copy(d, n, text[pos])`) recusado | 035 | 858 → 0 |
| 7 | Default de array (`const s[] = ""`) contava como obrigatório | 088 | 3 → 20 plugins |
| 8 | Cauda variádica vazia tratada como argumento faltante | 088 | 20 → 49 plugins |
| 9 | Nome do enum não registrado como constante (`new d[PlayerData]`) | 008 | 49 → 56 plugins |
| 10 | Pragmas de biblioteca com segundo nome (`#pragma defclasslib a b`) | 038 | 56 → 66 plugins |

Padrão observado: milhares de erros vinham de poucos descuidos pontuais, quase sempre
uma linha. O maior salto isolado foi o #8.

### Os 2 plugins restantes

Casos individuais, não mais padrões sistêmicos: 5× erro 001 (parser), 5× erro 008,
1× 050 (range), 1× 027 (char constant), e `create_entity`/`DispatchKeyValue` indefinidos
em `dod/stats.sma` e `dod/stats_logging.sma` — este último possivelmente **não é bug nosso**:
`dodx.inc` não inclui `engine.inc`, então o amxxpc real também falharia com este conjunto de
includes. **Não verificável sem o `amxxpc.exe`.**

### O que continua não provado

Os 66 `.amxx` gerados são estruturalmente válidos e passam pelo nosso próprio disassembler,
mas **nenhum foi carregado num servidor**, e **nenhuma saída foi comparada com o amxxpc**.
"Melhor que o amxxpc" está apoiado em 6 bugs do compilador original encontrados e corrigidos
(ver `docs/DIVERGENCES.md` §1) — não em medição de paridade.



