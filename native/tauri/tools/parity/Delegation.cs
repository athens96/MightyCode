// The delegation part of the parity checks (W7-A; `cargo run -p mighty-core --example parity -- delegation <in> <out>`): the round trip
// of the nine scripts/fixtures/delegation-record/*/profile/delegation-state.json files through DelegationFileStore and DelegationJson,
// and a vector corpus (request keys, ChildStates.After over the whole table, DelegationPruning on generated files, DelegationSidebar.Rows),
// each answered by MightyCode.Core. Not written yet: W7-A fills it.
namespace MightyCode.Parity;

internal static class DelegationParity
{
    public static int Run(string input, string output)
    {
        Console.Error.WriteLine($"delegation parity is not written yet (W7-A): {input} -> {output}");
        return 2;
    }
}
