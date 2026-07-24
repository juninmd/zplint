# Licensing — guia prático de engenharia (zpc)

Guia operacional para quem escreve código no `zpc`. Créditos formais e textos de licença
reproduzidos ficam em [`../ATTRIBUTION.md`](../ATTRIBUTION.md). **Este documento não é
aconselhamento jurídico** — descreve apenas o que os cabeçalhos das licenças dizem
literalmente e as regras de trabalho que derivamos disso.

## 1. Regra de ouro

> **Código GPL nunca é transcrito.** Nem copiado, nem traduzido linha-a-linha para Rust, nem
> "reescrito olhando para a tela". Formatos binários podem ser reimplementados a partir de
> fatos (magic, offsets, ordem de campos); implementações não.

Fato (layout de arquivo, valor de constante, código de erro) não é o mesmo que expressão
(o código que o produz). Portamos expressão só de fontes permissivas.

## 2. Tabela por componente

| Componente | Arquivo(s) upstream | Licença | O que podemos fazer |
|---|---|---|---|
| Núcleo do compilador Pawn | `libpc300/sc1.c`–`sc7.c`, `sc.h`, `sclist.c`, `scstate.c`, `scvars.c`, `scmemfil.c`, `sci18n.c`, `libpawnc.c`, `pawncc.c` | zlib-style (ITB CompuPhase) | **Portar livremente**, inclusive comercialmente. Obrigações: marcar como versão alterada, não se dizer autor do original, não remover o aviso. |
| Tabela de erros/warnings | `libpc300/sc5-in.scp` | zlib-style (CompuPhase) | **Reusar textos e códigos** para paridade de diagnósticos. |
| Sequências do peephole | `libpc300/sc7-in.scp` | zlib-style (CompuPhase) | **Portar.** |
| Header `.amx` / ABI da AMX | `libpc300/amx.h`, `amxdbg.h` (idem em `amxxpc/amx.cpp`, `amx.h`) | zlib-style (CompuPhase) | **Portar.** |
| `osdefs.h` | `libpc300/osdefs.h` | Só linha de copyright CompuPhase, **sem concessão explícita** | Não copiar o arquivo. Detecção de plataforma em Rust é `cfg!`, não precisamos dele. |
| `memfile` | `libpc300/memfile.c`, `memfile.h` | **GPLv3+ (AMX Mod X)** — dentro do libpc300 | **Não portar.** Substituir por `Vec<u8>`/`Cursor`. |
| Hash de símbolos | `libpc300/sp_symhash.c`, `.h` | **Sem cabeçalho de licença** | **Não portar.** Usar `HashMap` do Rust. Procedência indefinida = risco. |
| BinReloc | `libpc300/prefix.c`, `prefix.h` | Domínio público (declarado no arquivo) | Usável, mas desnecessário (`std::path`/`std::env`). |
| Container `.amxx` + driver | `amxxpc/amxxpc.cpp`, `amxxpc.h`, `Binary.cpp`, `Binary.h` | **GPLv3+ (AMX Mod X)** | **Não portar.** Reimplementar a partir de fatos do formato: magic, versão, cellsize=4, seções, compressão zlib. |

## 3. O que a licença zlib do Pawn exige — e o que NÃO exige

Texto integral em [`../ATTRIBUTION.md`](../ATTRIBUTION.md) §1.

**Exige (3 condições):**
1. Não deturpar a origem; não alegar que escrevemos o software original. (Reconhecimento na
   documentação é "apreciado, mas não obrigatório" — nós fazemos assim mesmo.)
2. Versões alteradas devem ser **claramente marcadas como tais** e não podem ser apresentadas
   como o software original. → `zpc` é declarado versão alterada.
3. O aviso **não pode ser removido ou alterado de nenhuma distribuição de fonte**.

**NÃO exige:**
- copyleft — o derivado pode ser proprietário ou ter qualquer licença;
- distribuir o código-fonte;
- reproduzir o aviso na documentação binária (a condição 3 fala de *source distribution*);
- usar o mesmo nome de licença.

**Ponto de julgamento (decisão do dono, não nossa):** um port para outra linguagem não é
literalmente uma "source distribution" do arquivo original. A leitura conservadora — e a que
adotamos — é manter o aviso em `ATTRIBUTION.md` e nos cabeçalhos dos arquivos derivados como se
a condição 3 se aplicasse. Se o dono quiser uma posição diferente, é decisão dele com apoio
jurídico.

## 4. Checklist para quem adiciona código portado

Antes de abrir PR com código derivado de upstream:

- [ ] Identifiquei o **arquivo upstream exato** de onde a lógica veio.
- [ ] Li o **cabeçalho desse arquivo** (não presumi pela pasta — `libpc300` tem arquivos GPL e
      arquivos sem licença).
- [ ] O cabeçalho é a licença zlib da CompuPhase → pode portar.
- [ ] É GPL, sem cabeçalho, ou só linha de copyright → **parar** e reimplementar de fatos, ou
      perguntar ao dono.
- [ ] Adicionei no topo do arquivo Rust um comentário no formato:
      `// Derivado de libpc300/<arquivo> (Pawn compiler, ITB CompuPhase, zlib). Versão ALTERADA — ver ATTRIBUTION.md.`
- [ ] Se o componente ainda não estiver na tabela §2 e em `ATTRIBUTION.md` §3, incluí lá.
- [ ] Se for reimplementação de formato (não port), o commit/PR diz **de onde vieram os fatos**
      (spec, hexdump, comportamento observado) e afirma que nenhum código GPL foi consultado
      linha-a-linha.

## 5. Decisão em aberto: licença do zplint

Estado atual: **sem arquivo `LICENSE` no repositório** e **sem campo `license` no
`Cargo.toml` raiz**. Sem isso, por padrão o código é "todos os direitos reservados" —
terceiros não têm permissão para usar, e publicar no crates.io exige `license` ou
`license-file`.

Restrição vinda do código zlib que passaremos a distribuir: a licença zlib é permissiva e
**compatível com praticamente qualquer escolha** — MIT, Apache-2.0, MIT/Apache dual (padrão
Rust), BSD, zlib, GPL, ou proprietária. Ela **não impõe** copyleft. O que ela impõe, em
qualquer cenário, são as 3 condições da §3 — que continuam valendo por cima da licença que o
dono escolher.

Consequência prática, sem recomendar nenhuma opção:

| Escolha | Efeito sobre as obrigações da §3 |
|---|---|
| MIT / Apache-2.0 / dual / BSD / zlib | Permitido. Obrigações da CompuPhase continuam e precisam do aviso em `ATTRIBUTION.md`. |
| GPLv3 | Permitido (zlib é compatível para dentro da GPL). Note que isso é escolha nossa, **não** obrigação herdada. |
| Proprietária / sem licença | Permitido pela zlib, mas hoje impede uso por terceiros e publicação no crates.io. |

Cuidado adicional: **nada disso muda se algum dia código GPL do AMX Mod X entrar no repo.**
Se entrar, a distribuição inteira passa a ter obrigações de copyleft. É exatamente por isso que
existe a regra da §1.

**Ações que só o dono pode decidir:**
1. Qual licença o zplint adota.
2. Criar o arquivo `LICENSE` correspondente na raiz.
3. Preencher `license = "..."` (ou `license-file`) no `Cargo.toml` raiz e nos crates de
   `crates/*`.

Este documento **não escolhe** por ele e nenhum arquivo `LICENSE` foi criado.
