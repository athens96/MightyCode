// The phone part of the parity checks (W8-A and W8-B): MightyCode.Core answering
//   push <in> <out>                    W8-A: MobilePushPlanner's messages and the hand-written push JSON for a vector corpus (scalars, C0
//                                      controls, '/', non-BMP); Rust compares the bytes.
//   pairing-url <in> <out>             W8-A: MobileIdentity.PairingUrl for given identities.
//   routes <profile> <requests> <out>  W8-B: MobileDesktopRouter over a DesktopService loaded from <profile> with no runs, answering each
//                                      GET of <requests>.jsonl (/m1/info, /m1/state, /m1/sessions/{id}, entries, commands, files); the
//                                      Rust headless desk answers the same, compared as JSON values, then as bytes.
// Not written yet: W8-A and W8-B fill it.
namespace MightyCode.Parity;

internal static class PhoneParity
{
    public static int Push(string input, string output)
    {
        Console.Error.WriteLine($"push parity is not written yet (W8-A): {input} -> {output}");
        return 2;
    }

    public static int PairingUrl(string input, string output)
    {
        Console.Error.WriteLine($"pairing-url parity is not written yet (W8-A): {input} -> {output}");
        return 2;
    }

    public static int Routes(string profile, string requests, string output)
    {
        Console.Error.WriteLine($"routes parity is not written yet (W8-B): {profile} {requests} -> {output}");
        return 2;
    }
}
