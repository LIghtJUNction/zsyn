#let x = 123 + 4 * 5
#let square(n) = n * n
#let values = (1, 2, 3)
#let config = (size: 12pt, enabled: true)
#for n in values [#square(n)]
#if x > 10 [large] else [small]
#set text(size: config.size)
#show heading: it => [*#it.body*]

