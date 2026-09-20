"use strict";
// Trailing slash selects the declared browser polyfill, not Node's builtin.
module.exports = require("./codec.cjs")(require("buffer/").Buffer);
