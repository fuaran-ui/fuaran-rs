// The byte-parity oracle for the chart data-table documents beside this script.
//
// Decodes every `*.json` node document in <dir> with the reference host's
// decoder and writes the reference SERVER renderer's HTML for it, rendered with
// no host sources, twice: under the default render policy (the chart table
// offered) to <dir>/<name>.reference.txt, and with the chart table turned off
// to <dir>/<name>.off.txt. Both are UTF-8 with no BOM and no trailing newline.
//
// Regenerate (from a fuaran-dotnet checkout at the reference revision):
//
//   dotnet build src/Fuaran.UI.Renderer.Server.Tests -c Release
//   dotnet fsi <this file> <fuaran-dotnet>/src/Fuaran.UI.Renderer.Server.Tests/bin/Release/net10.0 <dir>
//
// The test project's output is used because it carries the decoder and the
// server renderer in one folder.
open System
open System.IO
open System.Reflection
open Microsoft.FSharp.Reflection

let args = fsi.CommandLineArgs |> Array.skip 1
let binDir, dir = args[0], args[1]

AppDomain.CurrentDomain.add_AssemblyResolve (fun _ e ->
  let path = Path.Combine(binDir, AssemblyName(e.Name).Name + ".dll")
  if File.Exists path then Assembly.LoadFrom path else null)

let load (name: string) =
  Assembly.LoadFrom(Path.Combine(binDir, name + ".dll"))

let decodeNode =
  (load "Fuaran.UI.Ops").GetType("Fuaran.UI.Ops.JsonDecode").GetMethod("decodeNodeObj")

let renderModule = (load "Fuaran.UI.Renderer.Server").GetType("Fuaran.UI.Renderer.Server.Render")
let render = renderModule.GetMethod("render")
let renderWithPolicy = renderModule.GetMethod("renderWithPolicy")

let emptyRegistry =
  (load "Fuaran.UI.Renderer.Server").GetType("Fuaran.UI.Renderer.Server.Registry").GetProperty("empty").GetValue(null)

let core = load "Fuaran.UI.Renderer.Core"

let noSources =
  core.GetType("Fuaran.UI.Renderer.BindingResolver").GetProperty("empty").GetValue(null)

// `{ RenderPolicy.defaults with ChartTable = ChartTablePolicy.Off }`, by reflection.
let offPolicy =
  let defaults =
    core.GetTypes()
    |> Array.find (fun t -> t.FullName.EndsWith "RenderPolicyModule")
    |> fun m -> m.GetProperty("defaults").GetValue(null)

  let recordType = defaults.GetType()
  let fields = FSharpType.GetRecordFields recordType
  let values = FSharpValue.GetRecordFields defaults
  let i = fields |> Array.findIndex (fun f -> f.Name = "ChartTable")
  let off = FSharpType.GetUnionCases fields[i].PropertyType |> Array.find (fun c -> c.Name = "Off")
  values[i] <- FSharpValue.MakeUnion(off, [||])
  FSharpValue.MakeRecord(recordType, values)

let write (path: string) (html: string) =
  File.WriteAllBytes(path, Text.UTF8Encoding(false).GetBytes html)
  printfn "%s" path

for file in Directory.GetFiles(dir, "*.json") |> Array.sort do
  let result = decodeNode.Invoke(null, [| box (File.ReadAllText file) |])
  let resultType = result.GetType()

  if (resultType.GetProperty("Tag").GetValue(result) :?> int) <> 0 then
    failwithf "%s: decode failed: %A" file (resultType.GetProperty("ErrorValue").GetValue(result))

  let node = resultType.GetProperty("ResultValue").GetValue(result)
  let stem = Path.Combine(dir, Path.GetFileNameWithoutExtension file)
  write (stem + ".reference.txt") (render.Invoke(null, [| noSources; node |]) :?> string)
  write (stem + ".off.txt") (renderWithPolicy.Invoke(null, [| offPolicy; emptyRegistry; noSources; node |]) :?> string)
