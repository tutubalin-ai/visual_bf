A tiny copycat: echo stdin to stdout until end of input

Pipe some text into the visualizer to watch it get echoed back

,.              read one byte and print it
[               while the byte was not zero (end of input)
  ,.            read the next byte and print it
]
