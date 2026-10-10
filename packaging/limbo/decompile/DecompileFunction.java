// DecompileFunction: a Ghidra headless post-script. It writes the decompiled C of the
// functions named in its arguments to a file, so the caller does not depend on how
// headless mode formats script output.
//
// Arguments: OUTFILE FUNC [FUNC...]
//   FUNC is an address such as 0x401136 (the function containing it) or a function name.
//
// It only reads the program Ghidra has already imported and analysed. It never runs it.
//
//@category Limbo

import java.io.PrintWriter;

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;

public class DecompileFunction extends GhidraScript {
	private static final int TIMEOUT_SECONDS = 120;

	@Override
	protected void run() throws Exception {
		String[] args = getScriptArgs();
		if (args.length < 2) {
			printerr("usage: DecompileFunction.java OUTFILE FUNC [FUNC...]");
			return;
		}
		DecompInterface decompiler = new DecompInterface();
		try (PrintWriter out = new PrintWriter(args[0], "UTF-8")) {
			if (!decompiler.openProgram(currentProgram)) {
				out.println("could not open the program for decompiling: " + decompiler.getLastMessage());
				return;
			}
			for (int i = 1; i < args.length; i++) {
				String want = args[i];
				out.println("=== " + want + " ===");
				Function f = find(want);
				if (f == null) {
					out.println("no function found for " + want);
					continue;
				}
				out.println("// " + f.getName() + " @ " + f.getEntryPoint());
				DecompileResults r = decompiler.decompileFunction(f, TIMEOUT_SECONDS, monitor);
				if (r.decompileCompleted()) {
					out.println(r.getDecompiledFunction().getC());
				} else {
					out.println("decompile failed: " + r.getErrorMessage());
				}
			}
		} finally {
			decompiler.dispose();
		}
	}

	private Function find(String want) {
		if (want.startsWith("0x") || want.startsWith("0X")) {
			try {
				Address a = toAddr(Long.parseUnsignedLong(want.substring(2), 16));
				Function f = getFunctionContaining(a);
				return f != null ? f : getFunctionAt(a);
			} catch (NumberFormatException e) {
				return null;
			}
		}
		FunctionIterator it = currentProgram.getFunctionManager().getFunctions(true);
		while (it.hasNext()) {
			Function f = it.next();
			if (f.getName().equals(want)) {
				return f;
			}
		}
		return null;
	}
}
