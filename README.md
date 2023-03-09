![bp workflow](https://github.com/tommythorn/bp/actions/workflows/ci.yml/badge.svg)

# Silly Experiments With Branch Prediction

This project came about as I wanted to test various branch prediction
algorithms, notably GShare and YAGS.  It was also one of the earlier
of serious usage of Rust and as such it's probably chock-full of bad
style.  It could _certainly_ be generalized and cleaned up.  As it
happens, this project is also where I go experiment with stuff, like
most recently continuous integration.

# TL;DR

YAGS is awesome and beats GShare for the same bit-budget and is only
marginally more complicated (though enough that doing it in a single
stage may become a speed path).
